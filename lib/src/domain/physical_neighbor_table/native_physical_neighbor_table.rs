use std::process::Command;
use std::{collections::HashMap, net::Ipv6Addr};
use std::ops::Deref;

use crate::domain::{InMemoryPNTable, NetworkInterface, NodeId, PNTable, StateSeqNr};

/// A physical neighbor table backed by a [HashMap].
///
/// This wrapper limits the write access on the inner [HashMap] as the [StateSeqNr] has
/// to be updated every time the physical neighbors change.
#[derive(Debug)]
pub struct NativePNTable {
    map: InMemoryPNTable,
}

impl Default for NativePNTable {
    fn default() -> Self {
        Self::new()
    }
}

impl Deref for NativePNTable {
    type Target = HashMap<NodeId, NetworkInterface>;

    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

impl NativePNTable {
    pub fn new() -> Self {
        Self {
            map: InMemoryPNTable::new(),
        }
    }

    pub fn into_inner(self) -> HashMap<NodeId, NetworkInterface> {
        self.map.into_inner()
    }

    fn remove_from_routing_table(id: &NodeId, interface: &NetworkInterface) {
        let node_ip = Ipv6Addr::from(id).to_string();

        log::trace!(target: "physical_neighbor_table", "Trying to delete route to {:?} dev {:?}", node_ip, &interface.name);

        let output = Command::new("ip")
            .args([
                "route", "del", &node_ip, "dev", &interface.name,
            ])
            .output()
            .expect("failed to execute ip command");

        match output
            .status
            .code()
            .expect("ip command externally terminated")
        {
            0 => {
                log::trace!(target: "physical_neighbor_table", "Deleted route to {:?}", node_ip);
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
    }

    fn insert_into_routing_table(id: &NodeId, interface: &NetworkInterface) {
        let node_ip = Ipv6Addr::from(id).to_string();

        log::trace!(target: "physical_neighbor_table", "Trying to add route to {:?} dev {:?}", node_ip, &interface.name);

        let output = Command::new("ip")
            .args([
                "route", "add", &node_ip, "dev", &interface.name,
            ])
            .output()
            .expect("failed to execute ip command");

        match output
            .status
            .code()
            .expect("ip command externally terminated")
        {
            0 => {
                log::trace!(target: "physical_neighbor_table", "Added route to {:?}", node_ip);
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
    }
}

impl PNTable for NativePNTable {
    fn insert(&mut self, id: NodeId, interface: NetworkInterface) -> Option<NetworkInterface> {
        let result = self.map.insert(id.clone(), interface.clone());
        if let Some(interface_old) = result.clone() {
            if interface_old != interface {
                Self::remove_from_routing_table(&id, &interface);
            }
        }

        Self::insert_into_routing_table(&id, &interface);
        result
    }

    fn contains(&self, id: &NodeId) -> bool {
        self.map.contains_key(id)
    }

    fn state_seq_nr(&self) -> &StateSeqNr {
        self.map.state_seq_nr()
    }

    fn remove(&mut self, id: &NodeId) -> Option<NetworkInterface> {
        let result = self.map.remove(id);
        if let Some(interface) = result.clone() {
            Self::remove_from_routing_table(id, &interface);
        }

        result
    }
}

impl<'a> IntoIterator for &'a NativePNTable {
    type Item = (&'a NodeId, &'a NetworkInterface);
    type IntoIter = std::collections::hash_map::Iter<'a, NodeId, NetworkInterface>;

    fn into_iter(self) -> Self::IntoIter {
        self.map.iter()
    }
}
