//! Fast forwarding implementation using SRv6 encapsulation and an eBPF program.
//!
//! The main struct of this module is the [`Srv6FwdTables`].
//!
//! # Encapsulation
//!
//! Packets are encapsulated as described in section 5.2.1 of the [KIRA draft][kira_draft]:
//! an outer IPv6 header with the PathIP of the first path segment as destination, followed by
//! a Reduced Segment Routing Header ([RFC 8754]) with the PathIP of the second path segment,
//! if there is one. The encapsulation is done by the SRv6 support of the Linux kernel
//! (`seg6` routes).
//!
//! # PathIDs
//!
//! Intermediate nodes replace the PathIP in the destination address of the outer header in
//! place. As this is not part of the SRv6 behavior of the Linux kernel, an
//! [eBPF program](crate::ebpf) attached to the ingress of the underlay interfaces handles all
//! packets destined to PathIPs: it swaps PathIPs, advances to the next path segment and
//! removes the encapsulation at the end of the last path segment.
//!
//! # Requirements
//!
//! - Linux >= 6.0 (reduced SRv6 encapsulation), built with `CONFIG_IPV6_SEG6_LWTUNNEL`
//! - `CAP_NET_ADMIN` and `CAP_BPF` (or root)
//! - IPv6 forwarding enabled
//! - underlay interfaces with Ethernet framing
//!
//! [kira_draft]: https://datatracker.ietf.org/doc/draft-bless-rtgwg-kira/
//! [RFC 8754]: https://datatracker.ietf.org/doc/rfc8754/

use std::{
    collections::HashMap,
    error::Error,
    fmt::Debug,
    future::Future,
};

use futures::{
    StreamExt,
    channel::mpsc::{
        UnboundedReceiver,
        UnboundedSender,
    },
};
use kira_r2kad::domain::UnderlayNeighborUpdate;
use netlink_packet_route::RouteNetlinkMessage;
use netlink_proto::ConnectionHandle;
use tracing::Level;

use crate::{
    domain::{
        DecapsulationDestination,
        InterfaceId,
        IpPrefixes,
        NodeId,
        NodeIdEncapsulationEntry,
        NodeIdForwardingEntry,
        NodeIdSubnet,
        PathId,
        PathIdDecapsulationEntry,
        PathIdForwardingEntry,
        UnderlayNeighborId,
    },
    ebpf::{
        self,
        DropEvents,
        PathTable,
    },
    netlink::{
        RouteNetlink,
        remove_routes,
    },
    tables::{
        AsyncForwardingTables,
        AsyncNodeIdTable,
        AsyncPathIdTable,
        NodeIdEntry,
        PathIdEntry,
        native_tables::error::FwdTableError,
    },
    underlay::{
        UnderlayInformationProvider,
        UnderlayNeighborInformation,
    },
};

/// [AsyncForwardingTables] implementation using SRv6 encapsulation, backed by linux routing
/// tables and an eBPF program.
///
/// Also logs every change to the forwarding tables with log target `srv6_fwd_table`.
#[derive(Debug)]
pub struct Srv6FwdTables<I> {
    node_id_table: HashMap<NodeIdSubnet, NodeIdEntry>,
    path_id_table: HashMap<PathId, PathIdEntry>,

    routes: RouteNetlink,
    path_table: PathTable,
    /// Reports of packets dropped because of an unknown PathIP, until taken.
    drop_events: Option<DropEvents>,
    interface_id_table: HashMap<UnderlayNeighborId, InterfaceId>,
    underlay_information_provider: I,
}

impl<I> Srv6FwdTables<I> {
    /// Creates a SRv6 forwarding tables instance.
    ///
    /// The returned future is responsible for attaching the NodeIP and the eBPF program to
    /// upcoming interfaces. The eBPF program is detached from all interfaces when the future
    /// is dropped. NodeIPs and PathIPs are derived using the given `ip_prefixes`.
    ///
    /// The future passes all `underlay_updates` on to `prepared_updates` in their order, an
    /// [InterfaceUp](UnderlayNeighborUpdate::InterfaceUp) only after the eBPF program is
    /// attached to the interface: until then, packets to PathIPs arriving on the interface
    /// are not forwarded, so neighbors must not learn about the node via the interface.
    pub async fn new(
        root_id: NodeId,
        ip_prefixes: IpPrefixes,
        handle_rtnetlink: ConnectionHandle<RouteNetlinkMessage>,
        underlay_information: I,
        mut underlay_updates: UnboundedReceiver<UnderlayNeighborUpdate>,
        prepared_updates: UnboundedSender<UnderlayNeighborUpdate>,
    ) -> Result<(Self, impl Future<Output = ()> + 'static), FwdTableError> {
        // a previous instance (e.g., after a crash) may have left its routes behind
        remove_routes(&handle_rtnetlink, &ip_prefixes).await?;

        let (mut program, path_table, drop_events) = ebpf::load(&ip_prefixes)?;
        log::debug!(target: "srv6_fwd_table", "Loaded eBPF program successfully");

        let routes = RouteNetlink::new(handle_rtnetlink, ip_prefixes);
        let attaching = {
            let routes = routes.clone();
            async move {
                while let Some(update) = underlay_updates.next().await {
                    if let UnderlayNeighborUpdate::InterfaceUp(id) = update {
                        if let Err(e) = routes.attach_node_id_ip(&root_id, id).await {
                            log::error!(target: "srv6_fwd_table", "Attaching NodeIP to interface {id:?} failed: {e}");
                        }
                        if let Err(e) = program.attach(id) {
                            log::error!(target: "srv6_fwd_table", "Attaching eBPF program to interface {id:?} failed: {e}");
                        }
                    }
                    // the receiver may be gone on shutdown, the program stays attached until
                    // this future is dropped
                    let _ = prepared_updates.unbounded_send(update);
                }
            }
        };

        Ok((
            Self {
                routes,
                path_table,
                drop_events: Some(drop_events),
                node_id_table: Default::default(),
                path_id_table: Default::default(),
                interface_id_table: Default::default(),
                underlay_information_provider: underlay_information,
            },
            attaching,
        ))
    }
}

impl<I> Srv6FwdTables<I> {
    /// The reports of the packets the eBPF program dropped because of an unknown PathIP,
    /// see [DropEvents::listen]. Returns `None` after the first call.
    pub fn take_drop_events(&mut self) -> Option<DropEvents> {
        self.drop_events.take()
    }
}

impl<I> Srv6FwdTables<I>
where
    I: UnderlayInformationProvider<Information = UnderlayNeighborInformation> + Send + Debug,
    I::Error: Error + 'static,
{
    async fn underlay_information(
        &mut self,
        ulnid: &UnderlayNeighborId,
    ) -> Result<UnderlayNeighborInformation, FwdTableError> {
        self.underlay_information_provider
            .get_information(ulnid)
            .await
            .map_err(|e| FwdTableError::UnderlayInformationProviderErr(Box::new(e)))?
            .ok_or(FwdTableError::UnknownUnderlayNeighborId(*ulnid))
    }
}

impl<I> AsyncNodeIdTable for Srv6FwdTables<I>
where
    I: UnderlayInformationProvider<Information = UnderlayNeighborInformation> + Send + Debug,
    I::Error: Error + 'static,
{
    type Error = FwdTableError;

    #[tracing::instrument(level = Level::TRACE, target = "srv6_fwd_table", fields(destination = %entry.destination()))]
    async fn create(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error> {
        if self.node_id_table.contains_key(entry.destination()) {
            log::error!(target: "srv6_fwd_table", "entry already exists {entry:?}");
        }

        AsyncNodeIdTable::create_or_update(self, entry).await
    }

    #[tracing::instrument(level = Level::TRACE, target = "srv6_fwd_table", fields(destination = %entry.destination()))]
    async fn update(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error> {
        if !self.node_id_table.contains_key(entry.destination()) {
            log::error!(target: "srv6_fwd_table", "entry missing {entry:?}");
        }

        AsyncNodeIdTable::create_or_update(self, entry).await
    }

    #[tracing::instrument(level = Level::TRACE, target = "srv6_fwd_table", fields(destination = %entry.destination()))]
    async fn create_or_update(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error> {
        let destination = entry.destination().clone();
        if self.node_id_table.get(&destination) == Some(&entry) {
            return Ok(());
        }

        match &entry {
            NodeIdEntry::Forward(NodeIdForwardingEntry {
                destination,
                next_hop,
            }) => {
                log::trace!(target: "srv6_fwd_table", "Trying to replace neighbor route {destination} dst {next_hop:?}");
                let out_interface = self.underlay_information(next_hop).await?.interface_id;
                let _ = self.interface_id_table.insert(*next_hop, out_interface);

                self.routes
                    .replace_neighbor_route(destination, out_interface)
                    .await?;
            }
            NodeIdEntry::Encapsulate(NodeIdEncapsulationEntry {
                destination,
                out_path_id,
                second_path_id,
                next_hop,
            }) => {
                let next_hop = self.underlay_information(next_hop).await?;

                // The encapsulated packet is forwarded by the route of its first PathIP.
                // It is installed first, so that the encapsulation never lacks its route.
                log::trace!(target: "srv6_fwd_table", "Trying to replace route to {out_path_id:?} via {next_hop:?}");
                self.routes
                    .replace_via_route(out_path_id, &next_hop)
                    .await?;

                let segments: Vec<&PathId> =
                    std::iter::once(out_path_id).chain(second_path_id).collect();
                log::trace!(target: "srv6_fwd_table", "Trying to replace seg6 route {destination} segs {segments:?}");
                self.routes
                    .replace_seg6_route(destination, &segments, next_hop.interface_id)
                    .await?;
            }
        }

        self.node_id_table.insert(destination, entry);

        Ok(())
    }

    #[tracing::instrument(level = Level::TRACE, target = "srv6_fwd_table", skip(node_id), fields(destination=%node_id))]
    async fn remove(&mut self, node_id: &NodeIdSubnet) -> Result<Option<NodeIdEntry>, Self::Error> {
        let Some(removed) = self.node_id_table.remove(node_id) else {
            return Ok(None);
        };

        match &removed {
            NodeIdEntry::Forward(NodeIdForwardingEntry { next_hop, .. }) => {
                // only delete routes to subnets and not our neighbors
                if node_id.ipv6_subnet_prefix_length() != 128
                    && let Some(interface_id) = self.interface_id_table.get(next_hop)
                {
                    log::trace!(target: "srv6_fwd_table", "Trying to delete neighbor route {node_id} dst {next_hop:?}");
                    self.routes
                        .delete_neighbor_route(node_id, *interface_id)
                        .await?;
                }
            }
            NodeIdEntry::Encapsulate(_) => {
                log::trace!(target: "srv6_fwd_table", "Trying to delete seg6 route {node_id}");
                self.routes.delete_seg6_route(node_id).await?;

                // TODO: delete via routes
            }
        }

        Ok(Some(removed))
    }
}

impl<I> AsyncPathIdTable for Srv6FwdTables<I>
where
    I: UnderlayInformationProvider<Information = UnderlayNeighborInformation> + Send + Debug,
    I::Error: Error + 'static,
{
    type Error = FwdTableError;

    #[tracing::instrument(level = Level::TRACE, target = "srv6_fwd_table", fields(in_path_id = %entry.in_path_id()))]
    async fn create(&mut self, entry: PathIdEntry) -> Result<(), Self::Error> {
        if self.path_id_table.contains_key(entry.in_path_id()) {
            return Err(FwdTableError::EntryAlreadyExists(entry.to_string()));
        }

        AsyncPathIdTable::create_or_update(self, entry).await
    }

    #[tracing::instrument(level = Level::TRACE, target = "srv6_fwd_table", fields(in_path_id = %entry.in_path_id()))]
    async fn update(&mut self, entry: PathIdEntry) -> Result<(), Self::Error> {
        if self.path_id_table.contains_key(entry.in_path_id()) {
            AsyncPathIdTable::create_or_update(self, entry).await
        } else {
            Err(FwdTableError::EntryMissing(entry.to_string()))
        }
    }

    #[tracing::instrument(level = Level::TRACE, target = "srv6_fwd_table", fields(in_path_id = %entry.in_path_id()))]
    async fn create_or_update(&mut self, entry: PathIdEntry) -> Result<(), Self::Error> {
        let in_path_id = entry.in_path_id().clone();
        if self.path_id_table.get(&in_path_id) == Some(&entry) {
            return Ok(());
        }
        let ip_prefixes = *self.routes.ip_prefixes();
        let in_ip = ip_prefixes.path_ip(&in_path_id);

        match &entry {
            PathIdEntry::Forward(PathIdForwardingEntry {
                out_path_id,
                next_hop,
                ..
            }) => {
                let next_hop = self.underlay_information(next_hop).await?;

                // the route of the outgoing PathIP has to exist before packets are swapped to it
                log::trace!(target: "srv6_fwd_table", "Trying to create via route: {out_path_id:?} via {next_hop:?}");
                self.routes
                    .replace_via_route(out_path_id, &next_hop)
                    .await?;

                log::trace!(target: "srv6_fwd_table", "Trying to set swap entry: {entry:?}");
                self.path_table
                    .set_swap(in_ip, ip_prefixes.path_ip(out_path_id))?;
            }
            PathIdEntry::Decapsulate(PathIdDecapsulationEntry {
                next_hop: DecapsulationDestination::Local,
                ..
            }) => {
                log::trace!(target: "srv6_fwd_table", "Trying to set end entry: {entry:?}");
                self.path_table.set_end(in_ip)?;
            }
            PathIdEntry::Decapsulate(PathIdDecapsulationEntry {
                next_hop: DecapsulationDestination::UnderlayNeighbor(_),
                ..
            }) => {
                log::error!(target: "srv6_fwd_table", "Penultimate hop popping is currently not supported by the SRv6 forwarding tables: {entry:?}");
                return Ok(());
            }
        }

        self.path_id_table.insert(in_path_id, entry);
        Ok(())
    }

    #[tracing::instrument(level = Level::TRACE, target = "srv6_fwd_table", skip(path_id), fields(in_path_id=%path_id))]
    async fn remove(&mut self, path_id: &PathId) -> Result<Option<PathIdEntry>, Self::Error> {
        let Some(removed) = self.path_id_table.remove(path_id) else {
            return Ok(None);
        };

        log::trace!(target: "srv6_fwd_table", "Trying to remove entry from path table: {removed:?}");
        let in_ip = self.routes.ip_prefixes().path_ip(path_id);
        self.path_table.remove(in_ip)?;
        Ok(Some(removed))
    }
}

impl<I> AsyncForwardingTables for Srv6FwdTables<I>
where
    I: UnderlayInformationProvider<Information = UnderlayNeighborInformation> + Send + Debug,
    I::Error: Error + 'static,
{
}
