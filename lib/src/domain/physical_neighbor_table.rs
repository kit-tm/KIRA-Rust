use std::collections::HashMap;
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
        self.map.remove(id)
    }
}

impl<'a> IntoIterator for &'a PNTable {
    type Item = (&'a NodeId, &'a NetworkInterface);
    type IntoIter = std::collections::hash_map::Iter<'a, NodeId, NetworkInterface>;

    fn into_iter(self) -> Self::IntoIter {
        self.map.iter()
    }
}
