use crate::domain::{NetworkInterface, NodeId, StateSeqNr};
pub use native_physical_neighbor_table::NativePNTable;
pub use in_memory_physical_neighbor_table::InMemoryPNTable;

mod native_physical_neighbor_table;
mod in_memory_physical_neighbor_table;

// todo add doc and module doc
/// DOC WIP
pub trait PNTable {
    /// Adds a Mapping to the table returning the [NetworkInterface] previously mapped to the [NodeId].
    fn insert(&mut self, id: NodeId, interface: NetworkInterface) -> Option<NetworkInterface>;

    /// Returns if a Mapping for the [NodeId] is present in the [PNTable].
    fn contains(&self, id: &NodeId) -> bool;
    /// Returns the state sequence number.
    ///
    /// The state sequence number represents the number of connectivity changes in the
    /// direct physical neighborhood of a node.
    fn state_seq_nr(&self) -> &StateSeqNr;

    /// Removed a Mapping from the table returning that NetworkInterface the [NodeId] was mapped to.
    fn remove(&mut self, id: &NodeId) -> Option<NetworkInterface>;
}
