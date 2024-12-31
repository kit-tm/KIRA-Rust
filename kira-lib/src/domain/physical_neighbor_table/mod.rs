use crate::domain::{NodeId, StateSeqNr, UnderlayNeighborId};
pub use in_memory_physical_neighbor_table::InMemoryPNTable;

mod in_memory_physical_neighbor_table;

/// A [PNTable] models the physical neighbor table.
///
/// ## Invariants
///
/// * The output of [`state_seq_nr`][Self::state_seq_nr] should change every time the [PNTable] is mutated.
pub trait PNTable {
    /// Adds a Mapping to the table returning the [UnderlayNeighborId] previously mapped to the [NodeId].
    fn insert(&mut self, id: NodeId, ulnid: UnderlayNeighborId) -> Option<UnderlayNeighborId>;

    /// Returns if a Mapping for the [NodeId] is present in the [PNTable].
    fn contains(&self, id: &NodeId) -> bool;

    /// Returns the state sequence number.
    ///
    /// The state sequence number represents the number of connectivity changes in the
    /// direct physical neighborhood of a node.
    fn state_seq_nr(&self) -> &StateSeqNr;

    /// Removed a Mapping from the table returning that UnderlayNeighborId the [NodeId] was mapped to.
    fn remove(&mut self, id: &NodeId) -> Option<UnderlayNeighborId>;
}
