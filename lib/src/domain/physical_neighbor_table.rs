use std::collections::HashMap;
use std::ops::Deref;

use crate::domain::{NodeId, Port, StateSeqNr};

/// A physical neighbor table backed by a [HashMap].
///
/// This wrapper limits the write access on the inner [HashMap] as the [StateSeqNr] has
/// to be updated every time the physical neighbors change.
#[derive(Debug)]
pub struct PNTable {
    state_seq_nr: StateSeqNr,
    map: HashMap<NodeId, Port>,
}

impl Default for PNTable {
    fn default() -> Self {
        Self::new()
    }
}

impl Deref for PNTable {
    type Target = HashMap<NodeId, Port>;

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

    pub fn into_inner(self) -> HashMap<NodeId, Port> {
        self.map
    }

    /// Adds a Mapping to the table returning the [Port] previously mapped to the [NodeId].
    pub fn add(&mut self, id: NodeId, port: Port) -> Option<Port> {
        // No Update for entry => No Increase of StateSeqNr
        if let Some(true) = self
            .map
            .get(&id)
            .map(|existing_port| existing_port == &port)
        {
            return None;
        }

        let result = self.map.insert(id, port);
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
}

impl<'a> IntoIterator for &'a PNTable {
    type Item = (&'a NodeId, &'a Port);
    type IntoIter = std::collections::hash_map::Iter<'a, NodeId, Port>;

    fn into_iter(self) -> Self::IntoIter {
        self.map.iter()
    }
}
