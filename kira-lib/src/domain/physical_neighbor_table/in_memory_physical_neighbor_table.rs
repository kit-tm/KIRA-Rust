use std::collections::{hash_map::Entry, HashMap};
use std::ops::Deref;

use crate::domain::{NodeId, PNTable, StateSeqNr, UnderlayNeighborId};

/// A physical neighbor table backed by a [HashMap].
///
/// This wrapper limits the write access on the inner [HashMap] as the [StateSeqNr] has
/// to be updated every time the physical neighbors change.
#[derive(Debug)]
pub struct InMemoryPNTable {
    state_seq_nr: StateSeqNr,
    map: HashMap<NodeId, UnderlayNeighborId>,
}

impl Default for InMemoryPNTable {
    fn default() -> Self {
        Self::new()
    }
}

impl Deref for InMemoryPNTable {
    type Target = HashMap<NodeId, UnderlayNeighborId>;

    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

impl InMemoryPNTable {
    pub fn new() -> Self {
        Self {
            state_seq_nr: StateSeqNr::from(0),
            map: HashMap::new(),
        }
    }

    pub fn into_inner(self) -> HashMap<NodeId, UnderlayNeighborId> {
        self.map
    }

    #[cfg(test)]
    pub fn state_seq_nr_mut(&mut self) -> &mut StateSeqNr {
        &mut self.state_seq_nr
    }
}

impl PNTable for InMemoryPNTable {
    fn insert(&mut self, id: NodeId, ulnid: UnderlayNeighborId) -> Option<UnderlayNeighborId> {
        let entry = self.map.entry(id);
        let result = match entry {
            Entry::Occupied(mut entry) => {
                // No Update for entry => No Increase of StateSeqNr
                if *entry == ulnid {
                    return None;
                }
                Some(entry.insert(ulnid))
            }
            Entry::Vacant(mut vacant) => {
                vacant.insert(ulnid);
                None
            }
        };

        self.state_seq_nr += 1;
        return result;
    }

    fn contains(&self, id: &NodeId) -> bool {
        self.map.contains_key(id)
    }

    fn state_seq_nr(&self) -> &StateSeqNr {
        &self.state_seq_nr
    }

    fn remove(&mut self, id: &NodeId) -> Option<UnderlayNeighborId> {
        let result = self.map.remove(id);
        // test if we actually removed something => update ssn
        if result.is_some() {
            self.state_seq_nr += 1;
        }

        result
    }
}

impl<'a> IntoIterator for &'a InMemoryPNTable {
    type Item = (&'a NodeId, &'a UnderlayNeighborId);
    type IntoIter = std::collections::hash_map::Iter<'a, NodeId, UnderlayNeighborId>;

    fn into_iter(self) -> Self::IntoIter {
        self.map.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssn_on_insert() {
        let mut table = InMemoryPNTable::new();

        let id = NodeId::zero();
        let interface = UnderlayNeighborId::with_name("test");

        let before_ssn = table.state_seq_nr().clone();
        table.insert(id, interface);
        let after_ssn = table.state_seq_nr().clone();

        assert_ne!(
            before_ssn, after_ssn,
            "State sequence number didn't change after insertion."
        )
    }

    #[test]
    fn ssn_on_remove() {
        let mut table = InMemoryPNTable::new();

        let id = NodeId::zero();
        let interface = UnderlayNeighborId::with_name("test");

        table.insert(id.clone(), interface);

        let before_ssn = table.state_seq_nr().clone();
        table.remove(&id);
        let after_ssn = table.state_seq_nr().clone();

        assert_ne!(
            before_ssn, after_ssn,
            "State sequence number didn't change after removing."
        )
    }

    #[test]
    fn ssn_on_fake_remove() {
        let mut table = InMemoryPNTable::new();

        let id = NodeId::zero();
        let interface = UnderlayNeighborId::with_name("test");

        let before_ssn = table.state_seq_nr().clone();
        table.remove(&id);
        let after_ssn = table.state_seq_nr().clone();

        assert_eq!(
            before_ssn, after_ssn,
            "State sequence number did change but we didn't remove anything."
        )
    }
}
