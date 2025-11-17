use std::collections::{HashMap, hash_map::Entry};
use std::ops::Deref;

use crate::domain::{NodeId, SafeStateSeqNr, ULNTable, UnderlayNeighborId};

/// A underlay neighbor table backed by a [HashMap].
///
/// This wrapper limits the write access on the inner [HashMap] as the [SafeStateSeqNr] has
/// to be updated every time the underlay neighbors change.
#[derive(Debug)]
pub struct InMemoryULNTable {
    state_seq_nr: SafeStateSeqNr,
    map: HashMap<NodeId, UnderlayNeighborId>,
}

impl Default for InMemoryULNTable {
    fn default() -> Self {
        Self::new()
    }
}

impl Deref for InMemoryULNTable {
    type Target = HashMap<NodeId, UnderlayNeighborId>;

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

    pub fn into_inner(self) -> HashMap<NodeId, UnderlayNeighborId> {
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
                // No Update for entry => No Increase of StateSeqNr
                if entry.get() == &ulnid {
                    return None;
                }
                Some(entry.insert(ulnid))
            }
            Entry::Vacant(vacant) => {
                vacant.insert(ulnid);
                None
            }
        };

        if let Some(new_ssn) = (self.state_seq_nr + 1).value() {
            self.state_seq_nr = new_ssn;
        } else {
            todo!("implement reset of SafeStateSeqNr");
        }
        result
    }

    fn contains(&self, id: &NodeId) -> bool {
        self.map.contains_key(id)
    }

    fn state_seq_nr(&self) -> &SafeStateSeqNr {
        &self.state_seq_nr
    }

    fn remove(&mut self, id: &NodeId) -> Option<UnderlayNeighborId> {
        let result = self.map.remove(id);
        // test if we actually removed something => update ssn
        if result.is_some() {
            if let Some(new_ssn) = (self.state_seq_nr + 1).value() {
                self.state_seq_nr = new_ssn;
            } else {
                todo!("implement reset of SafeStateSeqNr");
            }
        }

        result
    }
}

impl<'a> IntoIterator for &'a InMemoryULNTable {
    type Item = (&'a NodeId, &'a UnderlayNeighborId);
    type IntoIter = std::collections::hash_map::Iter<'a, NodeId, UnderlayNeighborId>;

    fn into_iter(self) -> Self::IntoIter {
        self.map.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests;
    use super::*;

    #[test]
    fn ssn_on_insert() {
        let table = InMemoryULNTable::new();
        tests::ssn_on_insert(table);
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
}
