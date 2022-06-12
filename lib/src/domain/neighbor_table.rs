use super::{Contact, NodeId};

/// Represents a physical interface.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Interface {
    id: String,
}

/// Maps [NodeId]s to [Interface]s.
pub trait NeighborTable<const ID_SIZE: usize> {
    /// Returns the [Interface] for a [NodeId] if present.
    fn get(&self, id: &NodeId<ID_SIZE>) -> Option<&Interface>;
    /// Adds a Mapping to the table returning the [Interface] previously mapped to the [NodeId].
    fn add(&mut self, id: NodeId<ID_SIZE>, iface: Interface) -> Option<Interface>;
    /// Returns if a Mapping for the [NodeId] is present in the [NeighborTable].
    fn contains(&self, id: &NodeId<ID_SIZE>) -> bool;
}
