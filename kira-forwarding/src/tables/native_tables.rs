use std::collections::HashMap;
use std::ffi::OsStr;
use std::net::Ipv6Addr;

use crate::domain::NodeIdSubnet;
use crate::domain::PathId;
use crate::platform;
use crate::tables::{ForwardingTables, NodeIdEntry, NodeIdTable, PathIdEntry, PathIdTable};

/// Native linux [ForwardingTables] implementation backed by nftables and linux routing tables.
///
/// Also logs every change to the forwarding tables with log target `native_fwd_table`.
#[derive(Debug, Default)]
pub struct NativeFwdTables {
    node_id_table: HashMap<NodeIdSubnet, NodeIdEntry>,
    path_id_table: HashMap<PathId, PathIdEntry>,
}

impl NativeFwdTables {
    pub fn new<S: AsRef<OsStr>>(nftables_conf: S) -> Self {
        platform::create_kira_interface().expect("KIRA interface doesn't exist prior");
        platform::load_nft_config(nftables_conf).unwrap();
        log::debug!(target: "native_fwd_table", "Loaded nftables successfully");
        Self::default()
    }
}

impl Drop for NativeFwdTables {
    fn drop(&mut self) {
        platform::delete_kira_interface()
            .expect("KIRA interface should have been created at creation");
    }
}

impl NodeIdTable for NativeFwdTables {
    type Error = error::FwdTableError;

    fn create(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error> {
        log::debug!(target: "native_fwd_table", "CREATE {:?}", entry);

        let destination = match entry {
            NodeIdEntry::Forward(ref entry) => entry.destination.clone(),
            NodeIdEntry::Encapsulate(ref entry) => entry.destination.clone(),
        };

        if self.node_id_table.contains_key(&destination) {
            //return Err(error::FwdTableError::EntryAlreadyExists(destination.to_string()));
            log::error!(target: "native_fwd_table", "entry already exists {:?}", entry);
        }

        NodeIdTable::create_or_update(self, entry)
    }

    fn update(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error> {
        log::debug!(target: "native_fwd_table", "UPDATE {:?}", entry);

        let destination = match entry {
            NodeIdEntry::Forward(ref entry) => entry.destination.clone(),
            NodeIdEntry::Encapsulate(ref entry) => entry.destination.clone(),
        };

        if !self.node_id_table.contains_key(&destination) {
            //return Err(error::FwdTableError::EntryMissing(destination.to_string()));
            log::error!(target: "native_fwd_table", "entry missing {:?}", entry);
        }

        NodeIdTable::create_or_update(self, entry)
    }

    fn remove(&mut self, node_id: &NodeIdSubnet) -> Result<Option<NodeIdEntry>, Self::Error> {
        if let Some(removed) = self.node_id_table.remove(node_id) {
            let prefix_len = if node_id.prefix_length() == 0 || node_id.prefix_length() > 128 {
                128
            } else {
                16 + node_id.prefix_length()
            };

            match removed {
                NodeIdEntry::Forward(ref entry) => {
                    // only delete routes to subnets and not our neighbors
                    if prefix_len != 128 {
                        let node_ip = format!(
                            "{}/{}",
                            Ipv6Addr::from(entry.destination.node_id()),
                            prefix_len
                        );
                        log::trace!(target: "native_fwd_table", "Trying to delete neighbor route {:?} dst {:?}", &node_ip, &entry.out_interface.name);
                        platform::delete_neighbor_route(&node_ip, &entry.out_interface.name)
                            .unwrap();
                    }
                }
                NodeIdEntry::Encapsulate(ref entry) => {
                    let path_ip = Ipv6Addr::from(&entry.out_path_id).to_string();
                    let node_ip = format!(
                        "{}/{}",
                        Ipv6Addr::from(entry.destination.node_id()),
                        prefix_len
                    );

                    log::trace!(target: "native_fwd_table", "Trying to delete encap route {:?} dst {:?}", &node_ip, &path_ip);
                    platform::delete_encap_route(&node_ip, &path_ip).unwrap();
                }
            }

            Ok(Some(removed))
        } else {
            Ok(None)
        }
    }

    fn create_or_update(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error> {
        let destination = match entry {
            NodeIdEntry::Forward(ref entry) => entry.destination.clone(),
            NodeIdEntry::Encapsulate(ref entry) => entry.destination.clone(),
        };

        if self.node_id_table.get(&destination) == Some(&entry) {
            return Ok(());
        }

        let prefix_len = if destination.prefix_length() == 0 || destination.prefix_length() > 128 {
            128
        } else {
            16 + destination.prefix_length()
        };

        // TODO fix this
        // Because of how the routing table works, there can only be one entry per prefix_len != 128 (not completely correct but works for now)
        // these subnet entries may change their destination when the routing table grows, so remove the old ones first
        if prefix_len != 128 {
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
                NodeIdTable::remove(self, &old_entry)?;
            }
        }

        match entry {
            NodeIdEntry::Forward(ref entry) => {
                // known physical neighbors are already configured, so only configure new subnets
                if prefix_len != 128 {
                    let node_ip = format!(
                        "{}/{}",
                        Ipv6Addr::from(entry.destination.node_id()),
                        prefix_len
                    );
                    log::trace!(target: "native_fwd_table", "Trying to replace neighbor route {:?} dst {:?}", &node_ip, &entry.out_interface.name);
                    platform::replace_neighbor_route(&node_ip, &entry.out_interface.name).unwrap();
                }
            }
            NodeIdEntry::Encapsulate(ref entry) => {
                let path_ip = Ipv6Addr::from(&entry.out_path_id).to_string();
                let node_ip = format!(
                    "{}/{}",
                    Ipv6Addr::from(entry.destination.node_id()),
                    prefix_len
                );
                let next_hop_ip = Ipv6Addr::from(&entry.next_hop).to_string();

                log::trace!(target: "native_fwd_table", "Trying to replace encap route {:?} dst {:?}", &node_ip, &path_ip);
                platform::replace_encap_route(&node_ip, &path_ip).unwrap();

                // If the prefix length is != 128, a subnet route is configured.
                // This means that an already existing and configured path to a contact is used.
                // The via route to this contact is already configured.
                // To avoid unnecessary reconfiguration we don't configure the via route again.
                if prefix_len == 128 {
                    log::trace!(target: "native_fwd_table", "Trying to replace route to {:?} via {:?}", &path_ip, &next_hop_ip);
                    platform::replace_via_route(&path_ip, &next_hop_ip).unwrap();
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
}

impl PathIdTable for NativeFwdTables {
    type Error = error::FwdTableError;

    fn create(&mut self, entry: PathIdEntry) -> Result<(), Self::Error> {
        log::debug!(target: "native_fwd_table", "PathIdTable CREATE {:?}", entry);

        let in_path_id = match entry {
            PathIdEntry::Decapsulate(ref entry) => entry.in_path_id.clone(),
            PathIdEntry::Forward(ref entry) => entry.in_path_id.clone(),
        };

        if self.path_id_table.contains_key(&in_path_id) {
            return Err(error::FwdTableError::EntryAlreadyExists(entry.to_string()));
        }

        PathIdTable::create_or_update(self, entry)
    }

    fn update(&mut self, entry: PathIdEntry) -> Result<(), Self::Error> {
        log::trace!(target: "native_fwd_table", "PathIdTable UPDATE {:?}", entry);

        let in_path_id = match entry {
            PathIdEntry::Decapsulate(ref entry) => entry.in_path_id.clone(),
            PathIdEntry::Forward(ref entry) => entry.in_path_id.clone(),
        };

        if self.path_id_table.contains_key(&in_path_id) {
            PathIdTable::create_or_update(self, entry)
        } else {
            Err(error::FwdTableError::EntryMissing(entry.to_string()))
        }
    }

    fn create_or_update(&mut self, entry: PathIdEntry) -> Result<(), Self::Error> {
        let in_path_id = match entry {
            PathIdEntry::Decapsulate(ref entry) => entry.in_path_id.clone(),
            PathIdEntry::Forward(ref entry) => entry.in_path_id.clone(),
        };
        let out_ip = match entry {
            PathIdEntry::Decapsulate(ref entry) => Ipv6Addr::from(&entry.local_id),
            PathIdEntry::Forward(ref entry) => Ipv6Addr::from(&entry.out_path_id),
        };
        let next_hop = match entry {
            PathIdEntry::Forward(ref entry) => Some(Ipv6Addr::from(&entry.next_hop)),
            _ => None,
        };

        if let Some(old_entry) = self.path_id_table.get_mut(&in_path_id) {
            if old_entry == &entry {
                return Ok(());
            }
            log::trace!(target: "native_fwd_table", "Trying to update entry in forwardmap from {:?} to {:?}", old_entry, entry);
            *old_entry = entry;
            platform::update_forwarding_rule(Ipv6Addr::from(&in_path_id), out_ip).unwrap();
        } else {
            log::trace!(target: "native_fwd_table", "Trying to insert entry into forwardmap: {:?}", entry);

            platform::add_forwarding_rule(Ipv6Addr::from(&in_path_id), out_ip).unwrap();
            self.path_id_table.insert(in_path_id, entry);
        }

        if let Some(next_hop) = next_hop {
            log::trace!(target: "native_fwd_table", "Trying to create via route: {:?} via {:?}", out_ip, next_hop);
            platform::replace_via_route(&out_ip.to_string(), &next_hop.to_string()).unwrap();
        }
        Ok(())
    }

    fn remove(&mut self, path_id: &PathId) -> Result<Option<PathIdEntry>, Self::Error> {
        log::trace!(target: "native_fwd_table", "PathIdTable REMOVE {:?} -> ?", path_id);
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

impl ForwardingTables for NativeFwdTables {}

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
