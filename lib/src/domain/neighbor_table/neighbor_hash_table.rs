use std::collections::HashMap;
use std::ops::{Deref, DerefMut};

use crate::domain::{NeighborTable, NodeId, Port, StateSeqNr};

/// A NeighborTable backed by a [HashMap].
#[derive(Debug)]
pub struct NeighborHashTable {
    state_seq_nr: StateSeqNr,
    map: HashMap<NodeId, Port>,
}

impl Default for NeighborHashTable {
    fn default() -> Self {
        Self::new()
    }
}

impl Deref for NeighborHashTable {
    type Target = HashMap<NodeId, Port>;

    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

impl DerefMut for NeighborHashTable {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.map
    }
}

impl NeighborHashTable {
    pub fn new() -> Self {
        Self {
            state_seq_nr: StateSeqNr::from(0),
            map: HashMap::new(),
        }
    }

    pub fn into_inner(self) -> HashMap<NodeId, Port> {
        self.map
    }
}

impl<'a> IntoIterator for &'a NeighborHashTable {
    type Item = (&'a NodeId, &'a Port);
    type IntoIter = std::collections::hash_map::Iter<'a, NodeId, Port>;

    fn into_iter(self) -> Self::IntoIter {
        self.map.iter()
    }
}

impl NeighborTable for NeighborHashTable {
    fn get(&self, id: &NodeId) -> Option<&Port> {
        self.map.get(id)
    }

    fn add(&mut self, id: NodeId, iface: Port) -> Option<Port> {
        // No Update for entry => No Increase of StateSeqNr
        if let Some(true) = self.map.get(&id).map(|port| port == &iface) {
            return None;
        }

        let result = self.map.insert(id, iface);
        self.state_seq_nr += 1;
        result
    }

    fn contains(&self, id: &NodeId) -> bool {
        self.map.contains_key(id)
    }

    fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    fn len(&self) -> usize {
        self.map.len()
    }

    fn state_seq_nr(&self) -> &StateSeqNr {
        &self.state_seq_nr
    }
}
