use std::collections::HashMap;

use super::NodeId;

/// Represents a logical [Port] where a [Message] can be received from or sent to.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Port {
    All,
    Named(String),
}

impl Port {
    pub fn new(id: String) -> Self {
        Self::Named(id)
    }
}

/// Maps [NodeId]s to [Port]s.
pub trait NeighborTable {
    /// Returns the [Port] for a [NodeId] if present.
    fn get(&self, id: &NodeId) -> Option<&Port>;
    /// Adds a Mapping to the table returning the [Port] previously mapped to the [NodeId].
    fn add(&mut self, id: NodeId, iface: Port) -> Option<Port>;
    /// Returns if a Mapping for the [NodeId] is present in the [NeighborTable].
    fn contains(&self, id: &NodeId) -> bool;
    /// Returns if any neighbors are present.
    fn is_empty(&self) -> bool;
    /// Returns the number of neighbors.
    fn len(&self) -> usize;
}

impl NeighborTable for HashMap<NodeId, Port> {
    fn get(&self, id: &NodeId) -> Option<&Port> {
        HashMap::get(self, id)
    }

    fn add(&mut self, id: NodeId, iface: Port) -> Option<Port> {
        HashMap::insert(self, id, iface)
    }

    fn contains(&self, id: &NodeId) -> bool {
        HashMap::contains_key(self, id)
    }

    fn is_empty(&self) -> bool {
        HashMap::is_empty(self)
    }

    fn len(&self) -> usize {
        HashMap::len(self)
    }
}
