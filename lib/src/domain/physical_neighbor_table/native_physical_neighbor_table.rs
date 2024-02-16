use std::ops::Deref;
use std::{collections::HashMap, net::Ipv6Addr};

use crate::domain::{NetworkInterface, NodeId, StateSeqNr};
use crate::forwarding::platform;

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
}

impl PNTable for NativePNTable {
    insert(&mut self, id: NodeId, interface: NetworkInterface) -> Option<NetworkInterface> {
        platform::replace_neighbor_route(&Ipv6Addr::from(&id).to_string(), &interface.name).unwrap();
        self.map.insert(id, interface)
    }

    contains(&self, id: &NodeId) -> bool {
        self.map.contains_key(id)
    }

    state_seq_nr(&self) -> &StateSeqNr {
        self.map.state_seq_nr()
    }

    remove(&mut self, id: &NodeId) -> Option<NetworkInterface> {
        let result = self.map.remove(id);
        if let Some(ref interface) = result {
            platform::delete_neighbor_route(&Ipv6Addr::from(id).to_string(), interface.name).unwrap();
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
