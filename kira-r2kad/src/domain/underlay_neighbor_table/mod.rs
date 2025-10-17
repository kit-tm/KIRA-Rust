use crate::domain::{NodeId, SafeStateSeqNr, UnderlayNeighborId};

pub use in_memory_underlay_neighbor_table::InMemoryULNTable;

pub mod in_memory_underlay_neighbor_table;

/// A [ULNTable] models the underlay neighbor table.
///
/// ## Invariants
///
/// * The output of [`state_seq_nr`][Self::state_seq_nr] should change every time the [ULNTable] is mutated.
pub trait ULNTable {
    /// Adds a Mapping to the table returning the [UnderlayNeighborId] previously mapped to the [NodeId].
    fn insert(&mut self, id: NodeId, ulnid: UnderlayNeighborId) -> Option<UnderlayNeighborId>;

    /// Returns if a Mapping for the [NodeId] is present in the [ULNTable].
    fn contains(&self, id: &NodeId) -> bool;

    /// Returns the state sequence number.
    ///
    /// The state sequence number represents the number of connectivity changes in the
    /// direct underlay neighborhood of a node.
    fn state_seq_nr(&self) -> &SafeStateSeqNr;

    /// Removed a Mapping from the table returning that UnderlayNeighborId the [NodeId] was mapped to.
    fn remove(&mut self, id: &NodeId) -> Option<UnderlayNeighborId>;
}

#[cfg(test)]
mod tests {
    use crate::domain::ConnectionId;
    use crate::domain::InterfaceId;

    use super::*;

    pub fn ssn_on_insert<P: ULNTable>(mut table: P) {
        let id = NodeId::zero();
        let neighbor = {
            let interface_id = InterfaceId::try_from(42).unwrap();
            let conn_id = ConnectionId::from(42);

            UnderlayNeighborId {
                interface_id,
                connection_id: conn_id,
            }
        };

        let before_ssn = *table.state_seq_nr();
        table.insert(id, neighbor);
        let after_ssn = *table.state_seq_nr();

        assert_ne!(
            before_ssn, after_ssn,
            "State sequence number didn't change after insertion."
        )
    }

    pub fn ssn_on_remove<P: ULNTable>(mut table: P) {
        let id = NodeId::zero();
        let neighbor = {
            let interface_id = InterfaceId::try_from(42).unwrap();
            let conn_id = ConnectionId::from(42);

            UnderlayNeighborId {
                interface_id,
                connection_id: conn_id,
            }
        };

        table.insert(id, neighbor);

        let before_ssn = *table.state_seq_nr();
        table.remove(&id);
        let after_ssn = *table.state_seq_nr();

        assert_ne!(
            before_ssn, after_ssn,
            "State sequence number didn't change after removing."
        )
    }

    pub fn ssn_on_fake_remove<P: ULNTable>(mut table: P) {
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
