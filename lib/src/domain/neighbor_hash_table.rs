use std::collections::HashMap;

use super::{Interface, NeighborTable, NodeId};

#[derive(Debug)]
pub struct NeighborHashTable<const ID_SIZE: usize> {
    table: HashMap<NodeId<ID_SIZE>, Interface>,
}

impl<const ID_SIZE: usize> Default for NeighborHashTable<ID_SIZE> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const ID_SIZE: usize> NeighborHashTable<ID_SIZE> {
    /// Creates a new empty [NeighborHashTable].
    pub fn new() -> Self {
        Self {
            table: HashMap::new(),
        }
    }
}

impl<const ID_SIZE: usize> NeighborTable<ID_SIZE> for NeighborHashTable<ID_SIZE> {
    fn get(&self, id: &NodeId<ID_SIZE>) -> Option<&Interface> {
        self.table.get(id)
    }

    fn add(&mut self, id: NodeId<ID_SIZE>, iface: Interface) -> Option<Interface> {
        self.table.insert(id, iface)
    }

    fn contains(&self, id: &NodeId<ID_SIZE>) -> bool {
        self.table.contains_key(id)
    }
}

impl<'a, const ID_SIZE: usize> IntoIterator for &'a NeighborHashTable<ID_SIZE> {
    type Item = (&'a NodeId<ID_SIZE>, &'a Interface);

    type IntoIter = std::collections::hash_map::Iter<'a, NodeId<ID_SIZE>, Interface>;

    fn into_iter(self) -> Self::IntoIter {
        self.table.iter()
    }
}
