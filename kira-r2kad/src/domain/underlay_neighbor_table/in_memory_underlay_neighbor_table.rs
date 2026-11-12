use std::{
    collections::{
        HashMap,
        hash_map::Entry,
    },
    ops::Deref,
};

use crate::domain::{
    NodeId,
    SafeStateSeqNr,
    ULNState,
    ULNTable,
    UnderlayNeighborId,
};

/// An underlay neighbor table backed by a [HashMap].
///
/// This wrapper limits the write access on the inner [HashMap] as the [SafeStateSeqNr] has
/// to be updated every time the underlay neighbors change.
#[derive(Debug)]
pub struct InMemoryULNTable {
    state_seq_nr: SafeStateSeqNr,
    map: HashMap<NodeId, (UnderlayNeighborId, ULNState)>,
}

impl Default for InMemoryULNTable {
    fn default() -> Self {
        Self::new()
    }
}

impl Deref for InMemoryULNTable {
    type Target = HashMap<NodeId, (UnderlayNeighborId, ULNState)>;

    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

impl InMemoryULNTable {
    pub fn new() -> Self {
        Self {
            state_seq_nr: SafeStateSeqNr::try_from(1)
                .expect("1 should be a safe state sequence number"),
            map: HashMap::new(),
        }
    }

    pub fn into_inner(self) -> HashMap<NodeId, (UnderlayNeighborId, ULNState)> {
        self.map
    }

    #[cfg(test)]
    pub fn state_seq_nr_mut(&mut self) -> &mut SafeStateSeqNr {
        &mut self.state_seq_nr
    }
}

impl ULNTable for InMemoryULNTable {
    fn insert(&mut self, id: NodeId, ulnid: UnderlayNeighborId) -> Option<UnderlayNeighborId> {
        let entry = self.map.entry(id);
        let result = match entry {
            Entry::Occupied(mut entry) => {
                // No Update for entry
                if entry.get().0 == ulnid {
                    return None;
                }
                Some(entry.insert((ulnid, ULNState::Tentative)))
            }
            Entry::Vacant(vacant) => {
                vacant.insert((ulnid, ULNState::Tentative));
                None
            }
        };

        result.map(|x| x.0)
    }

    fn confirm(&mut self, id: NodeId, ulnid: UnderlayNeighborId) -> bool {
        let map_entry = self.map.entry(id);
        match map_entry {
            Entry::Occupied(mut entry) => {
                // No Update for entry => No Increase of StateSeqNr
                match entry.get() {
                    // nothing to update
                    (existing_ulnid, ULNState::Bidirectional) if *existing_ulnid == ulnid => false,
                    // update to bidirectional and increase the sequence number
                    (existing_ulnid, ULNState::Tentative) if *existing_ulnid == ulnid => {
                        entry.insert((ulnid, ULNState::Bidirectional));
                        // wraps around, other nodes handle the lower number like a restart
                        self.state_seq_nr = self.state_seq_nr + 1;
                        true
                    }
                    _ => {
                        panic!(
                            "Internal errror - Trying to confirm entry for NodeID {} with new ulnid {} w/o inserting it first",
                            id, ulnid
                        );
                    }
                }
            }
            Entry::Vacant(_) => {
                // for a confirm we expect that there exists a prior entry for that ulnid
                panic!(
                    "Internal errror - Trying to confirm formerly unknown entry for NodeID {} and ulnid {}",
                    id, ulnid
                );
            }
        }
    }

    fn contains(&self, id: &NodeId) -> bool {
        self.map.contains_key(id)
    }

    fn is_bidirectional(&self, id: &NodeId) -> bool {
        matches!(self.map.get(id), Some((_, ULNState::Bidirectional)))
    }

    fn state_seq_nr(&self) -> &SafeStateSeqNr {
        &self.state_seq_nr
    }

    fn remove(&mut self, id: &NodeId) -> Option<UnderlayNeighborId> {
        let result = self.map.remove(id);
        // test if we actually removed an entry with bidirectional connectivity => update ssn
        // otherwise we do not update the ssn
        if let Some((_, uln_state)) = result
            && matches!(uln_state, ULNState::Bidirectional)
        {
            // wraps around, other nodes handle the lower number like a restart
            self.state_seq_nr = self.state_seq_nr + 1;
        }

        result.map(|x| x.0)
    }

    fn size(&self) -> usize {
        self.map.len()
    }
}

impl<'a> IntoIterator for &'a InMemoryULNTable {
    type IntoIter = std::collections::hash_map::Iter<'a, NodeId, (UnderlayNeighborId, ULNState)>;
    type Item = (&'a NodeId, &'a (UnderlayNeighborId, ULNState));

    fn into_iter(self) -> Self::IntoIter {
        self.map.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        super::tests,
        *,
    };

    #[test]
    fn ssn_wraps_around_after_the_largest_value() {
        let mut table = InMemoryULNTable::new();
        *table.state_seq_nr_mut() = SafeStateSeqNr::try_from(u32::MAX - 1).unwrap();
        let ulnid = UnderlayNeighborId {
            interface_id: crate::domain::InterfaceId::try_from(1).unwrap(),
            connection_id: crate::domain::ConnectionId(1),
        };
        let node = NodeId::with_msb(2);
        table.insert(node, ulnid);

        assert!(table.confirm(node, ulnid));

        assert_eq!(*table.state_seq_nr(), SafeStateSeqNr::MIN);
    }

    #[test]
    fn ssn_on_insert() {
        let table = InMemoryULNTable::new();
        tests::ssn_on_insert(table);
    }

    #[test]
    fn ssn_on_confirm() {
        let table = InMemoryULNTable::new();
        tests::ssn_on_confirm(table);
    }

    #[test]
    #[should_panic]
    fn ssn_on_wrong_confirm() {
        let table = InMemoryULNTable::new();
        tests::ssn_on_wrong_confirm(table);
    }

    #[test]
    fn ssn_on_remove() {
        let table = InMemoryULNTable::new();
        tests::ssn_on_remove(table);
    }

    #[test]
    fn ssn_on_fake_remove() {
        let table = InMemoryULNTable::new();
        tests::ssn_on_fake_remove(table);
    }

    #[test]
    fn is_bidirectional_only_after_confirm() {
        let mut table = InMemoryULNTable::new();
        let id = NodeId::ONE;
        let ulnid = UnderlayNeighborId {
            interface_id: crate::domain::InterfaceId::try_from(1).unwrap(),
            connection_id: crate::domain::ConnectionId::from(1),
        };

        assert!(!table.is_bidirectional(&id));

        table.insert(id, ulnid);
        assert!(table.contains(&id));
        assert!(
            !table.is_bidirectional(&id),
            "tentative entry is not bidirectional"
        );

        table.confirm(id, ulnid);
        assert!(table.is_bidirectional(&id));

        table.remove(&id);
        assert!(!table.is_bidirectional(&id));
    }
}
