use crate::domain::{NodeId, StateSeqNr, UnderlayNeighborId};
pub use in_memory_underlay_neighbor_table::InMemoryPNTable;

mod in_memory_underlay_neighbor_table;

/// A [UNTable] models the underlay neighbor table.
///
/// ## Invariants
///
/// * The output of [`state_seq_nr`][Self::state_seq_nr] should change every time the [PNTable] is mutated.
pub trait UNTable {
    /// Adds a Mapping to the table returning the [UnderlayNeighborId] previously mapped to the [NodeId].
    fn insert(&mut self, id: NodeId, ulnid: UnderlayNeighborId) -> Option<UnderlayNeighborId>;

    /// Returns if a Mapping for the [NodeId] is present in the [PNTable].
    fn contains(&self, id: &NodeId) -> bool;

    /// Returns the state sequence number.
    ///
    /// The state sequence number represents the number of connectivity changes in the
    /// direct underlay neighborhood of a node.
    fn state_seq_nr(&self) -> &StateSeqNr;

    /// Removed a Mapping from the table returning that UnderlayNeighborId the [NodeId] was mapped to.
    fn remove(&mut self, id: &NodeId) -> Option<UnderlayNeighborId>;
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use super::*;

    pub fn ssn_on_insert<P: UNTable>(mut table: P) {
        let id = NodeId::zero();
        let neighbor = NonZeroUsize::new(42).unwrap().into();

        let before_ssn = *table.state_seq_nr();
        table.insert(id, neighbor);
        let after_ssn = *table.state_seq_nr();

        assert_ne!(
            before_ssn, after_ssn,
            "State sequence number didn't change after insertion."
        )
    }

    pub fn ssn_on_remove<P: UNTable>(mut table: P) {
        let id = NodeId::zero();
        let neighbor = NonZeroUsize::new(42).unwrap().into();

        table.insert(id, neighbor);

        let before_ssn = *table.state_seq_nr();
        table.remove(&id);
        let after_ssn = *table.state_seq_nr();

        assert_ne!(
            before_ssn, after_ssn,
            "State sequence number didn't change after removing."
        )
    }

    pub fn ssn_on_fake_remove<P: UNTable>(mut table: P) {
        let id = NodeId::zero();

        let before_ssn = *table.state_seq_nr();
        table.remove(&id);
        let after_ssn = *table.state_seq_nr();

        assert_eq!(
            before_ssn, after_ssn,
            "State sequence number did change but we didn't remove anything."
        )
    }
}
