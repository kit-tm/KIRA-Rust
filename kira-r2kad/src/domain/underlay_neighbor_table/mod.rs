pub mod in_memory_underlay_neighbor_table;
#[doc(inline)]
pub use in_memory_underlay_neighbor_table::InMemoryULNTable;

pub mod observable_underlay_neighbor_table;
#[doc(inline)]
pub use observable_underlay_neighbor_table::ObservableULNTable;

use crate::domain::{
    NodeId,
    SafeStateSeqNr,
    UnderlayNeighborId,
};

/// A [ULNTable] models the underlay neighbor table.
///
/// ## Invariants
///
/// * The output of [`state_seq_nr`][Self::state_seq_nr] should change every time the [ULNTable] is changed.
pub trait ULNTable {
    /// Adds a tentative Mapping to the table returning the [UnderlayNeighborId] previously mapped to the [NodeId].
    fn insert(&mut self, id: NodeId, ulnid: UnderlayNeighborId) -> Option<UnderlayNeighborId>;

    /// confirms a Mapping to actually work in a bidirectional manner
    /// returns true if the sequence number state changed and false in case the state did not change.
    /// A neighbor that isn't in the table, or is in it with another [UnderlayNeighborId], is not confirmed.
    fn confirm(&mut self, id: NodeId, ulnid: UnderlayNeighborId) -> bool;

    /// Returns if a Mapping for the [NodeId] is present in the [ULNTable].
    ///
    /// This includes tentative mappings, whose link is only known to work towards us.
    fn contains(&self, id: &NodeId) -> bool;

    /// Returns if a Mapping for the [NodeId] is present and confirmed to work in both directions.
    fn is_bidirectional(&self, id: &NodeId) -> bool;

    /// Returns the state sequence number.
    ///
    /// The state sequence number represents the number of connectivity changes in the
    /// direct underlay neighborhood of a node.
    fn state_seq_nr(&self) -> &SafeStateSeqNr;

    /// Removed a Mapping from the table returning that UnderlayNeighborId the [NodeId] was mapped to.
    fn remove(&mut self, id: &NodeId) -> Option<UnderlayNeighborId>;

    /// Return number of underlay neighbors
    fn size(&self) -> usize;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        ConnectionId,
        InterfaceId,
    };

    pub fn ssn_on_insert<P: ULNTable>(mut table: P) {
        let id = NodeId::ZERO;
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

        assert_eq!(
            before_ssn, after_ssn,
            "State sequence number did change after first insertion."
        )
    }

    pub fn ssn_on_confirm<P: ULNTable>(mut table: P) {
        let id = NodeId::ZERO;
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
        table.confirm(id, neighbor);
        let after_ssn = *table.state_seq_nr();

        assert_ne!(
            before_ssn, after_ssn,
            "State sequence number did not change after first commit."
        );
        // confirming an already confirmed ULN should not change the SSN
        table.confirm(id, neighbor);
        let same_ssn = *table.state_seq_nr();
        assert_eq!(
            after_ssn, same_ssn,
            "State sequence number did change after second commit."
        );
    }

    pub fn ssn_on_wrong_confirm<P: ULNTable>(mut table: P) {
        let id = NodeId::ZERO;
        let neighbor = {
            let interface_id = InterfaceId::try_from(42).unwrap();
            let conn_id = ConnectionId::from(42);

            UnderlayNeighborId {
                interface_id,
                connection_id: conn_id,
            }
        };
        let new_neighbor = {
            let interface_id = InterfaceId::try_from(43).unwrap();
            let conn_id = ConnectionId::from(43);

            UnderlayNeighborId {
                interface_id,
                connection_id: conn_id,
            }
        };

        table.insert(id, neighbor);
        let ssn = *table.state_seq_nr();
        // a message via another underlay link doesn't confirm the stored one
        assert!(!table.confirm(id, new_neighbor));
        assert!(!table.is_bidirectional(&id));
        assert_eq!(*table.state_seq_nr(), ssn);

        // nor does one from a neighbor that isn't in the table
        assert!(!table.confirm(NodeId::with_lsb(7), neighbor));
        assert!(!table.contains(&NodeId::with_lsb(7)));
        assert_eq!(*table.state_seq_nr(), ssn);

        // the stored link can still be confirmed
        assert!(table.confirm(id, neighbor));
    }

    pub fn ssn_on_remove<P: ULNTable>(mut table: P) {
        let id = NodeId::ZERO;
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

        assert_eq!(
            before_ssn, after_ssn,
            "State sequence number should not change after removing an unconfirmed ULN"
        );

        table.insert(id, neighbor);
        table.confirm(id, neighbor);

        let confirm_ssn = *table.state_seq_nr();
        assert_ne!(
            after_ssn, confirm_ssn,
            "State sequence number should change after confirming the ULN"
        );

        table.remove(&id);
        let after_ssn = *table.state_seq_nr();

        assert_ne!(
            confirm_ssn, after_ssn,
            "State sequence number didn't change after removing."
        )
    }

    pub fn ssn_on_fake_remove<P: ULNTable>(mut table: P) {
        let id = NodeId::ZERO;

        let before_ssn = *table.state_seq_nr();
        table.remove(&id);
        let after_ssn = *table.state_seq_nr();

        assert_eq!(
            before_ssn, after_ssn,
            "State sequence number did change but we didn't remove anything."
        )
    }
}
