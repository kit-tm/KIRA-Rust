use std::process::Command;
use std::{collections::HashMap, net::Ipv6Addr};
use std::ops::Deref;

use crate::domain::{NetworkInterface, NodeId, StateSeqNr};

/// A physical neighbor table backed by a [HashMap].
///
/// This wrapper limits the write access on the inner [HashMap] as the [StateSeqNr] has
/// to be updated every time the physical neighbors change.
#[derive(Debug)]
pub struct PNTable {
    state_seq_nr: StateSeqNr,
    map: HashMap<NodeId, NetworkInterface>,
}

impl Default for PNTable {
    fn default() -> Self {
        Self::new()
    }
}

impl Deref for PNTable {
    type Target = HashMap<NodeId, NetworkInterface>;

    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

impl PNTable {
    pub fn new() -> Self {
        Self {
            state_seq_nr: StateSeqNr::from(0),
            map: HashMap::new(),
        }
    }

    pub fn into_inner(self) -> HashMap<NodeId, NetworkInterface> {
        self.map
    }

    /// Adds a Mapping to the table returning the [NetworkInterface] previously mapped to the [NodeId].
    pub fn insert(&mut self, id: NodeId, interface: NetworkInterface) -> Option<NetworkInterface> {
        // No Update for entry => No Increase of StateSeqNr
        if let Some(true) = self
            .map
            .get(&id)
            .map(|existing_port| existing_port == &interface)
        {
            return None;
        }

        if let Some(interface_old) = self.map.get(&id) {
            if interface_old != &interface {
                self.remove_from_routing_table(&id, &interface);
            }
        }
        self.insert_into_routing_table(&id, &interface);

        let result = self.map.insert(id, interface);
        self.state_seq_nr += 1;
        result
    }
    /// Returns if a Mapping for the [NodeId] is present in the [PNTable].
    pub fn contains(&self, id: &NodeId) -> bool {
        self.map.contains_key(id)
    }
    /// Returns the state sequence number.
    ///
    /// The state sequence number represents the number of connectivity changes in the
    /// direct physical neighborhood of a node.
    pub fn state_seq_nr(&self) -> &StateSeqNr {
        &self.state_seq_nr
    }
    /// Removed a Mapping from the table returning that NetworkInterface the [NodeId] was mapped to.
    pub fn remove(&mut self, id: &NodeId) -> Option<NetworkInterface> {
        if let Some(interface) = self.map.get(id) {
            self.remove_from_routing_table(id, interface);
        }

        self.map.remove(id)
    }

    fn remove_from_routing_table(&self, id: &NodeId, interface: &NetworkInterface) {
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

    fn insert_into_routing_table(&self, id: &NodeId, interface: &NetworkInterface) {
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

impl<'a> IntoIterator for &'a PNTable {
    type Item = (&'a NodeId, &'a NetworkInterface);
    type IntoIter = std::collections::hash_map::Iter<'a, NodeId, NetworkInterface>;

    fn into_iter(self) -> Self::IntoIter {
        self.map.iter()
    }
}
