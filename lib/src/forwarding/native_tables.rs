use std::collections::HashMap;
use std::net::Ipv6Addr;
use std::process::Command;

use crate::domain::path_id::PathId;
use crate::domain::NodeId;
use crate::forwarding::{ForwardingTables, NodeIdEntry, NodeIdTable, PathIdEntry, PathIdTable};

/// Native linux [ForwardingTables] implementation backed by nftables and linux routing tables.
///
/// Also logs every change to the forwarding tables with log target `native_fwd_table`.
#[derive(Debug, Default)]
pub struct NativeFwdTables {
    node_id_table: HashMap<NodeId, NodeIdEntry>,
    path_id_table: HashMap<PathId, PathIdEntry>,
}

impl NativeFwdTables {
    pub fn new() -> Self {
        let output = Command::new("nft")
            .args(["-f", "/r2kad-daemon/nftables.conf"])
            .output()
            .unwrap();
        match output.status.code().expect("failed to execute nft command") {
            0 => log::debug!(target: "native_fwd_table", "Loaded nftables successfully"),
            _status => {
                log::debug!(target: "native_fwd_table", "Loaded nftables config with status code {:?} and error message {:?}", _status, String::from_utf8_lossy(&output.stderr))
            }
        }

        Self::default()
    }
}

impl NodeIdTable for NativeFwdTables {
    type Error = error::FwdTableError;

    fn create(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error> {
        if self.node_id_table.contains_key(&entry.destination) {
            return Err(error::FwdTableError::EntryAlreadyExists);
        }

        if let Some(path_id) = &entry.out_path_id {
            let path_ip = Ipv6Addr::from(path_id).to_string();
            let node_ip = Ipv6Addr::from(&entry.destination).to_string();

            let next_hop_ip = Ipv6Addr::from(&entry.next_hop).to_string();

            let output = Command::new("ip")
                .args([
                    "-6", "route", "add", &node_ip, "encap", "ip6", "dst", &path_ip, "dev", "kira",
                ])
                .output()
                .expect("failed to execute ip command");

            match output
                .status
                .code()
                .expect("ip command externally terminated")
            {
                0 => {
                    log::trace!(target: "native_fwd_table", "Created {:?}", entry);
                    self.node_id_table.insert(entry.destination.clone(), entry);
                }
                1 => panic!(
                    "ip: syntax error: Err:\n{}\nOut:\n{}",
                    String::from_utf8_lossy(&output.stderr),
                    String::from_utf8_lossy(&output.stdout)
                ),
                2 => panic!(
                    "ip: kernel error: Err:\n{}\nOut:\n{}",
                    String::from_utf8_lossy(&output.stderr),
                    String::from_utf8_lossy(&output.stdout)
                ),
                _ => panic!(
                    "ip: unknown error: Err:\n{}\nOut:\n{}",
                    String::from_utf8_lossy(&output.stderr),
                    String::from_utf8_lossy(&output.stdout)
                ),
            }

            let output = Command::new("ip")
                .args(["-6", "route", "add", &path_ip, "via", &next_hop_ip])
                .output()
                .expect("failed to execute ip command");

            match output
                .status
                .code()
                .expect("ip command externally terminated")
            {
                0 => {
                    log::trace!(target: "native_fwd_table", "Added route to {:?} via {:?}", &path_ip, &next_hop_ip);
                }
                1 => panic!(
                    "ip: syntax error: Err:\n{}\nOut:\n{}",
                    String::from_utf8_lossy(&output.stderr),
                    String::from_utf8_lossy(&output.stdout)
                ),
                2 => panic!(
                    "ip: kernel error: Err:\n{}\nOut:\n{}",
                    String::from_utf8_lossy(&output.stderr),
                    String::from_utf8_lossy(&output.stdout)
                ),
                _ => panic!(
                    "ip: unknown error: Err:\n{}\nOut:\n{}",
                    String::from_utf8_lossy(&output.stderr),
                    String::from_utf8_lossy(&output.stdout)
                ),
            }
        } else {
            // No Path exists, forward to physical neighbor instead:
            // This is handled automatically by configuring all network interfaces
            // to allow forwarding and configuring routes to physical neighbors.
        }

        Ok(())
    }

    fn update(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error> {
        if let Some(old_entry) = self.node_id_table.get_mut(&entry.destination) {
            if let Some(path_id) = &entry.out_path_id {
                let path_ip = Ipv6Addr::from(path_id).to_string();
                let node_ip = Ipv6Addr::from(&entry.destination).to_string();

                let next_hop_ip = Ipv6Addr::from(&entry.next_hop).to_string();

                let output = Command::new("ip")
                    .args([
                        "-6", "route", "change", &node_ip, "encap", "ip6", "dst", &path_ip, "dev",
                        "kira",
                    ])
                    .output()
                    .expect("failed to execute ip command");

                match output
                    .status
                    .code()
                    .expect("ip command externally terminated")
                {
                    0 => {
                        log::trace!(target: "native_fwd_table", "Updated old: {:?}, new: {:?}", &old_entry, &entry);
                        *old_entry = entry;
                    }
                    1 => panic!(
                        "ip: syntax error: Err:\n{}\nOut:\n{}",
                        String::from_utf8_lossy(&output.stderr),
                        String::from_utf8_lossy(&output.stdout)
                    ),
                    2 => panic!(
                        "ip: kernel error: Err:\n{}\nOut:\n{}",
                        String::from_utf8_lossy(&output.stderr),
                        String::from_utf8_lossy(&output.stdout)
                    ),
                    _ => panic!(
                        "ip: unknown error: Err:\n{}\nOut:\n{}",
                        String::from_utf8_lossy(&output.stderr),
                        String::from_utf8_lossy(&output.stdout)
                    ),
                }

                let output = Command::new("ip")
                    .args(["-6", "route", "change", &path_ip, "via", &next_hop_ip])
                    .output()
                    .expect("failed to execute ip command");

                match output
                    .status
                    .code()
                    .expect("ip command externally terminated")
                {
                    0 => {
                        log::trace!(target: "native_fwd_table", "Changed route to {:?} via {:?}", &path_ip, &next_hop_ip);
                    }
                    1 => panic!(
                        "ip: syntax error: Err:\n{}\nOut:\n{}",
                        String::from_utf8_lossy(&output.stderr),
                        String::from_utf8_lossy(&output.stdout)
                    ),
                    2 => panic!(
                        "ip: kernel error: Err:\n{}\nOut:\n{}",
                        String::from_utf8_lossy(&output.stderr),
                        String::from_utf8_lossy(&output.stdout)
                    ),
                    _ => panic!(
                        "ip: unknown error: Err:\n{}\nOut:\n{}",
                        String::from_utf8_lossy(&output.stderr),
                        String::from_utf8_lossy(&output.stdout)
                    ),
                }
            } else {
                // No Path exists, forward to physical neighbor instead:
                // This is handled automatically by configuring all network interfaces
                // to allow forwarding and configuring routes to physical neighbors.
            }

            Ok(())
        } else {
            Err(error::FwdTableError::EntryMissing)
        }
    }

    fn remove(&mut self, node_id: &NodeId) -> Result<Option<NodeIdEntry>, Self::Error> {
        if let Some(removed) = self.node_id_table.remove(node_id) {
            if let Some(path_id) = &removed.out_path_id {
                // Update Path in routing table

                let path_ip = Ipv6Addr::from(path_id).to_string();
                let node_ip = Ipv6Addr::from(&removed.destination).to_string();

                let output = Command::new("ip")
                    .args([
                        "-6", "route", "del", &node_ip, "encap", "ip6", "dst", &path_ip, "dev",
                        "kira",
                    ])
                    .output()
                    .expect("failed to execute ip command");

                match output
                    .status
                    .code()
                    .expect("ip command externally terminated")
                {
                    0 => {
                        log::trace!(target: "native_fwd_table", "Removed {:?}", removed);
                    }
                    1 => panic!(
                        "ip: syntax error: Err:\n{}\nOut:\n{}",
                        String::from_utf8_lossy(&output.stderr),
                        String::from_utf8_lossy(&output.stdout)
                    ),
                    2 => panic!(
                        "ip: kernel error: Err:\n{}\nOut:\n{}",
                        String::from_utf8_lossy(&output.stderr),
                        String::from_utf8_lossy(&output.stdout)
                    ),
                    _ => panic!(
                        "ip: unknown error: Err:\n{}\nOut:\n{}",
                        String::from_utf8_lossy(&output.stderr),
                        String::from_utf8_lossy(&output.stdout)
                    ),
                }
            } else {
                // No Path exists, forward to physical neighbor instead
                // This is not implemented yet, see nftables.conf
                todo!()
            }

            Ok(Some(removed))
        } else {
            Ok(None)
        }
    }
}

impl PathIdTable for NativeFwdTables {
    type Error = error::FwdTableError;

    fn create(&mut self, entry: PathIdEntry) -> Result<(), Self::Error> {
        if self.path_id_table.contains_key(&entry.in_path_id) {
            return Err(error::FwdTableError::EntryAlreadyExists);
        }

        let in_path_ip = Ipv6Addr::from(&entry.in_path_id).to_string();
        let out_path_ip = Ipv6Addr::from(&entry.out_path_id).to_string();

        let output = Command::new("nft")
            .args([
                "add",
                "element",
                "ip6",
                "kira",
                "forwardmap",
                &format!("{{\"{}\" : \"{}\"}}", in_path_ip, out_path_ip),
            ])
            .output()
            .expect("failed to execute nft command");

        match output
            .status
            .code()
            .expect("nft command externally terminated")
        {
            0 => {
                log::trace!(target: "native_fwd_table", "Created {:?}", entry);
                self.path_id_table.insert(entry.in_path_id.clone(), entry);
            }
            3 => {
                panic!(
                    "nft: unable to open netlink socket: Err\n{}\nOut:\n{}",
                    String::from_utf8_lossy(&output.stderr),
                    String::from_utf8_lossy(&output.stdout)
                )
            }
            _ => {
                panic!(
                    "nft: unknown error: Err\n{}\nOut:\n{}",
                    String::from_utf8_lossy(&output.stderr),
                    String::from_utf8_lossy(&output.stdout)
                )
            }
        }

        Ok(())
    }

    fn update(&mut self, entry: PathIdEntry) -> Result<(), Self::Error> {
        if let Some(_) = self.path_id_table.get_mut(&entry.in_path_id) {
            // nft does not support updating elements

            PathIdTable::remove(self, &entry.in_path_id)?;

            PathIdTable::create(self, entry)?;

            Ok(())
        } else {
            Err(error::FwdTableError::EntryMissing)
        }
    }

    fn remove(&mut self, path_id: &PathId) -> Result<Option<PathIdEntry>, Self::Error> {
        if let Some(removed) = self.path_id_table.remove(path_id) {
            let in_path_ip = Ipv6Addr::from(path_id).to_string();

            let output = Command::new("nft")
                .args([
                    "delete",
                    "element",
                    "ip6",
                    "kira",
                    "forwardmap",
                    &format!("{{\"{}\"}}", in_path_ip),
                ])
                .output()
                .expect("failed to execute nft command");

            match output
                .status
                .code()
                .expect("nft command externally terminated")
            {
                0 => {
                    log::trace!(target: "native_fwd_table", "Removed {:?}", removed)
                }
                3 => {
                    panic!(
                        "nft: unable to open netlink socket: Err\n{}\nOut:\n{}",
                        String::from_utf8_lossy(&output.stderr),
                        String::from_utf8_lossy(&output.stdout)
                    )
                }
                _ => {
                    panic!(
                        "nft: unknown error: Err\n{}\nOut:\n{}",
                        String::from_utf8_lossy(&output.stderr),
                        String::from_utf8_lossy(&output.stdout)
                    )
                }
            }

            Ok(Some(removed))
        } else {
            Ok(None)
        }
    }
}

impl ForwardingTables for NativeFwdTables {}

pub mod error {
    use std::error::Error;
    use std::fmt::{Display, Formatter};

    #[derive(Debug)]
    pub enum FwdTableError {
        EntryAlreadyExists,
        EntryMissing,
    }

    impl Display for FwdTableError {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::EntryAlreadyExists => write!(f, "Entry already exists"),
                Self::EntryMissing => write!(f, "Entry with id doesn't exist"),
            }
        }
    }

    impl Error for FwdTableError {}
}
