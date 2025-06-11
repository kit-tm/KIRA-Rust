//! Fast forwarding implementation utilizing the native routing of the linux kernel.
//!
//! The main struct of this module is [NativeFwdTables].

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fmt::Debug;
use std::future::Future;
use std::net::Ipv6Addr;

use futures::channel::mpsc::UnboundedReceiver;
use futures::StreamExt;
use netlink_packet_route::RouteNetlinkMessage;
use netlink_proto::ConnectionHandle;
use tracing::{field, Level, Span};

use crate::domain::{
    DecapsulationDestination, NodeIdEncapsulationEntry, NodeIdForwardingEntry,
    PathIdDecapsulationEntry, PathIdForwardingEntry,
};
use crate::domain::{InterfaceId, NodeId, NodeIdSubnet, PathId, UnderlayNeighborId};
use crate::netlink::ForwardingRtNetlink;
use crate::platform;
use crate::tables::{
    AsyncForwardingTables, AsyncNodeIdTable, AsyncPathIdTable, NodeIdEntry, PathIdEntry,
};
use crate::underlay::{UnderlayInformationProvider, UnderlayNeighborInformation};
use kira_r2kad::domain::UnderlayNeighborUpdate;

/// Native linux [ForwardingTables] implementation backed by nftables and linux routing tables.
///
/// Also logs every change to the forwarding tables with log target `native_fwd_table`.
#[derive(Debug)]
pub struct NativeFwdTables<I> {
    root_id: NodeId,

    node_id_table: HashMap<NodeIdSubnet, NodeIdEntry>,
    path_id_table: HashMap<PathId, PathIdEntry>,

    netlink: ForwardingRtNetlink,
    interface_id_table: HashMap<UnderlayNeighborId, InterfaceId>,
    underlay_information_provider: I,
}

impl<I> NativeFwdTables<I> {
    /// Create a new forwarding tables instance.
    ///
    /// The returned future is responsible for attaching IPv6s on upcoming interfaces.
    pub async fn new<S: AsRef<OsStr>>(
        root_id: NodeId,
        nftables_conf: S,
        handle: ConnectionHandle<RouteNetlinkMessage>,
        underlay_information: I,
        mut underlay_updates: UnboundedReceiver<UnderlayNeighborUpdate>,
    ) -> (Self, impl Future<Output = ()> + 'static) {
        let netlink = ForwardingRtNetlink::new(handle)
            .await
            .expect("KIRA interface should successfully be created");
        platform::load_nft_config(nftables_conf).unwrap();
        log::debug!(target: "native_fwd_table", "Loaded nftables successfully");

        let attaching_ips = {
            let mut netlink = netlink.clone();
            async move {
                while let Some(update) = underlay_updates.next().await {
                    if let UnderlayNeighborUpdate::InterfaceUp(id) = update {
                        if let Err(e) = netlink.attach_node_id_ip(&root_id, id).await {
                            log::error!(target: "native_fwd_table", "Attaching to interface {:?} faile: {}", id, e);
                        }
                    }
                }
            }
        };

        (
            Self {
                root_id,
                netlink,
                node_id_table: Default::default(),
                path_id_table: Default::default(),
                interface_id_table: Default::default(),
                underlay_information_provider: underlay_information,
            },
            attaching_ips,
        )
    }
}

impl<I> Drop for NativeFwdTables<I> {
    fn drop(&mut self) {
        // FIXME: cleanup interfaces and routes
    }
}

impl<I> AsyncNodeIdTable for NativeFwdTables<I>
where
    I: UnderlayInformationProvider<Information = UnderlayNeighborInformation> + Send + Debug,
    I::Error: Debug,
{
    type Error = error::FwdTableError;

    #[tracing::instrument(level = Level::TRACE, target = "native_fwd_table", fields(destination = %entry.destination(), out_path_id=field::Empty))]
    async fn create(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error> {
        _ne_record_out_path_id(&entry);

        let destination = entry.destination();
        if self.node_id_table.contains_key(destination) {
            //return Err(error::FwdTableError::EntryAlreadyExists(destination.to_string()));
            log::error!(target: "native_fwd_table", "entry already exists {:?}", entry);
        }

        AsyncNodeIdTable::create_or_update(self, entry).await
    }

    #[tracing::instrument(level = Level::TRACE, target = "native_fwd_table", fields(destination = %entry.destination()))]
    async fn update(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error> {
        _ne_record_out_path_id(&entry);

        let destination = entry.destination();
        if !self.node_id_table.contains_key(destination) {
            //return Err(error::FwdTableError::EntryMissing(destination.to_string()));
            log::error!(target: "native_fwd_table", "entry missing {:?}", entry);
        }

        AsyncNodeIdTable::create_or_update(self, entry).await
    }

    #[tracing::instrument(level = Level::TRACE, target = "native_fwd_table", fields(destination = %entry.destination(), out_path_id=field::Empty))]
    async fn create_or_update(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error> {
        _ne_record_out_path_id(&entry);

        let destination = entry.destination().clone();
        let prefix_length = destination.ipv6_subnet_prefix_length();

        if self.node_id_table.get(&destination) == Some(&entry) {
            return Ok(());
        }

        // TODO fix this
        // Because of how the routing table works, there can only be one entry per prefix_len != 128 (not completely correct but works for now)
        // these subnet entries may change their destination when the routing table grows, so remove the old ones first
        if prefix_length != 128 {
            log::debug!(target: "native_fwd_table", "Checking for prefix entry change {:?}", entry);
            if let Some(old_entry) = self
                .node_id_table
                .keys()
                .find(|e| {
                    e.prefix_length() == destination.prefix_length()
                        && e.node_id() != destination.node_id()
                })
                .cloned()
            {
                log::debug!(target: "native_fwd_table", "Prefix entry changed from {} to {}", old_entry, entry);
                AsyncNodeIdTable::remove(self, &old_entry).await?;
            }
        }

        match &entry {
            NodeIdEntry::Forward(NodeIdForwardingEntry {
                destination,
                next_hop,
            }) => {
                log::trace!(target: "native_fwd_table", "Trying to replace neighbor route {} dst {:?}", destination, next_hop);
                let out_interface = self
                    .underlay_information_provider
                    .get_information(next_hop)
                    .await
                    .expect("next_hop ulnid is known") // FIXME: panic, understand how this can be
                    .interface_id;
                let _ = self.interface_id_table.insert(*next_hop, out_interface);

                self.netlink
                    .replace_neighbor_route(destination, out_interface)
                    .await
                    .unwrap();
            }
            NodeIdEntry::Encapsulate(NodeIdEncapsulationEntry {
                destination,
                out_path_id,
                next_hop,
            }) => {
                log::trace!(target: "native_fwd_table", "Trying to replace encap route {:?} dst {:?}", destination, out_path_id);
                self.netlink
                    .replace_encap_route(destination, out_path_id)
                    .await
                    .unwrap();

                // If the prefix length is != 128, a subnet route is configured.
                // This means that an already existing and configured path to a contact is used.
                // The via route to this contact is already configured.
                // To avoid unnecessary reconfiguration we don't configure the via route again.
                if prefix_length == 128 {
                    log::trace!(target: "native_fwd_table", "Trying to replace route to {:?} via {:?}", &out_path_id, &next_hop);
                    let next_hop = self
                        .underlay_information_provider
                        .get_information(next_hop)
                        .await
                        .expect("next_hop ulnid is known");
                    self.netlink
                        .replace_via_route(out_path_id, &next_hop)
                        .await
                        .unwrap();
                }
            }
        }

        if let Some(old_entry) = self.node_id_table.get_mut(&destination) {
            *old_entry = entry;
        } else {
            self.node_id_table.insert(destination, entry);
        }

        Ok(())
    }

    #[tracing::instrument(level = Level::TRACE, target = "native_fwd_table", skip(node_id), fields(destination=%node_id, out_path_id=field::Empty))]
    async fn remove(&mut self, node_id: &NodeIdSubnet) -> Result<Option<NodeIdEntry>, Self::Error> {
        if let Some(removed) = self.node_id_table.remove(node_id) {
            match &removed {
                NodeIdEntry::Forward(NodeIdForwardingEntry { next_hop, .. }) => {
                    // only delete routes to subnets and not our neighbors
                    if node_id.ipv6_subnet_prefix_length() != 128 {
                        log::trace!(target: "native_fwd_table", "Trying to delete neighbor route {} dst {:?}", node_id, next_hop);
                        let interface_id = *self
                            .interface_id_table
                            .get(next_hop)
                            .expect("interface id of underlay neighbor should be known");
                        let (node_ip, prefix) = node_id.to_ipv6_subnet();
                        self.netlink
                            .delete_neighbor_route(&node_ip, prefix, interface_id)
                            .await
                            .unwrap();
                    }
                }
                NodeIdEntry::Encapsulate(NodeIdEncapsulationEntry { out_path_id, .. }) => {
                    log::trace!(target: "native_fwd_table", "Trying to delete encap route {} dst {:?}", node_id, out_path_id);
                    self.netlink
                        .delete_encap_route(node_id, out_path_id)
                        .await
                        .unwrap();

                    // FIXME: delete via routes
                }
            }

            Ok(Some(removed))
        } else {
            Ok(None)
        }
    }
}

fn _ne_record_out_path_id(entry: &NodeIdEntry) {
    let span = Span::current();
    if span.is_disabled() {
        return;
    }

    if let Some(out_path_id) = entry.out_path_id() {
        span.record("out_path_id", format!("{}", out_path_id));
    }
}

impl<I> AsyncPathIdTable for NativeFwdTables<I>
where
    I: UnderlayInformationProvider<Information = UnderlayNeighborInformation> + Send + Debug,
    I::Error: Debug,
{
    type Error = error::FwdTableError;

    #[tracing::instrument(level = Level::TRACE, target = "native_fwd_table", fields(in_path_id = %entry.in_path_id(), out_path_id=field::Empty))]
    async fn create(&mut self, entry: PathIdEntry) -> Result<(), Self::Error> {
        _pe_record_out_path_id(&entry);

        let in_path_id = entry.in_path_id();
        if self.path_id_table.contains_key(in_path_id) {
            return Err(error::FwdTableError::EntryAlreadyExists(entry.to_string()));
        }

        AsyncPathIdTable::create_or_update(self, entry).await
    }

    #[tracing::instrument(level = Level::TRACE, target = "native_fwd_table", fields(in_path_id = %entry.in_path_id(), out_path_id=field::Empty))]
    async fn update(&mut self, entry: PathIdEntry) -> Result<(), Self::Error> {
        _pe_record_out_path_id(&entry);

        let in_path_id = entry.in_path_id();
        if self.path_id_table.contains_key(in_path_id) {
            AsyncPathIdTable::create_or_update(self, entry).await
        } else {
            Err(error::FwdTableError::EntryMissing(entry.to_string()))
        }
    }

    #[tracing::instrument(level = Level::TRACE, target = "native_fwd_table", fields(in_path_id = %entry.in_path_id(), out_path_id=field::Empty))]
    async fn create_or_update(&mut self, entry: PathIdEntry) -> Result<(), Self::Error> {
        _pe_record_out_path_id(&entry);

        let in_path_id = entry.in_path_id();
        let (out_ip, via) = match &entry {
            PathIdEntry::Decapsulate(PathIdDecapsulationEntry {
                next_hop: DecapsulationDestination::UnderlayNeighbor(ulnid),
                ..
            }) => {
                log::error!(target: "native_fwd_table", "Penultimate hop popping is currently not supported by the native forwarding tables: {:?}", entry);
                (
                    self.underlay_information_provider
                        .get_information(ulnid)
                        .await
                        .unwrap()
                        .ll_ipv6,
                    None,
                )
            }
            PathIdEntry::Decapsulate(PathIdDecapsulationEntry {
                next_hop: DecapsulationDestination::Local,
                ..
            }) => (Ipv6Addr::from(&self.root_id), None),
            PathIdEntry::Forward(PathIdForwardingEntry {
                out_path_id,
                next_hop,
                ..
            }) => {
                let next_hop = self
                    .underlay_information_provider
                    .get_information(next_hop)
                    .await
                    .unwrap();
                (out_path_id.into(), Some((next_hop, out_path_id.clone())))
            }
        };
        let in_ip = Ipv6Addr::from(in_path_id);

        if let Some(old_entry) = self.path_id_table.get_mut(in_path_id) {
            if old_entry == &entry {
                return Ok(());
            }
            log::trace!(target: "native_fwd_table", "Trying to update entry in forwardmap from {:?} to {:?}", old_entry, entry);
            *old_entry = entry;
            platform::update_forwarding_rule(in_ip, out_ip).unwrap();
        } else {
            log::trace!(target: "native_fwd_table", "Trying to insert entry into forwardmap: {:?}", entry);

            platform::add_forwarding_rule(in_ip, out_ip).unwrap();
            self.path_id_table.insert(in_path_id.clone(), entry);
        }

        if let Some((next_hop, out_path_id)) = via {
            log::trace!(target: "native_fwd_table", "Trying to create via route: {:?} via {:?}", out_ip, next_hop);
            self.netlink
                .replace_via_route(&out_path_id, &next_hop)
                .await
                .unwrap();
        }
        Ok(())
    }

    #[tracing::instrument(level = Level::TRACE, target = "native_fwd_table", skip(path_id), fields(in_path_id=%path_id))]
    async fn remove(&mut self, path_id: &PathId) -> Result<Option<PathIdEntry>, Self::Error> {
        if let Some(removed) = self.path_id_table.remove(path_id) {
            let in_path_ip = Ipv6Addr::from(path_id);

            log::trace!(target: "native_fwd_table", "Trying to remove entry from forwardmap: {:?}", removed);
            platform::delete_forwarding_rule(in_path_ip).unwrap();
            Ok(Some(removed))
        } else {
            Ok(None)
        }
    }
}

fn _pe_record_out_path_id(entry: &PathIdEntry) {
    let span = Span::current();
    if span.is_disabled() {
        return;
    }

    if let Some(out_path_id) = entry.out_path_id() {
        span.record("out_path_id", format!("{}", out_path_id));
    }
}

impl<I> AsyncForwardingTables for NativeFwdTables<I>
where
    I: UnderlayInformationProvider<Information = UnderlayNeighborInformation> + Send + Debug,
    I::Error: Debug,
{
}

#[allow(missing_docs)]
pub mod error {
    use derive_more::derive::Display;
    use std::error::Error;

    #[derive(Debug, Display)]
    pub enum FwdTableError {
        #[display("Entry {_0} already exists")]
        EntryAlreadyExists(String),
        #[display("Entry with id {_0} doesn't exist")]
        EntryMissing(String),
    }

    impl Error for FwdTableError {}
}
