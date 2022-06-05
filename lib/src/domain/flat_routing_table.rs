use crate::domain::{Bucket, Contact, NodeId, RoutingTable};

/// A [RoutingTable] implemented as flat array of [Bucket]s.
#[derive(Debug)]
pub struct FlatRoutingTable<const ID_SIZE: usize> {
    buckets: Vec<Bucket<ID_SIZE>>,
}

impl<const ID_SIZE: usize> FlatRoutingTable<ID_SIZE> {
    /// Creates a [RoutingTable] with 0 capacity.
    pub const fn new() -> Self {
        Self {
            buckets: Vec::new(),
        }
    }

    /// Create a [RoutingTable] with the capacity of its maximum possible number of buckets.
    ///
    /// That is equal to the [NodeId] Size in Bits.
    pub fn with_full_capacity() -> Self {
        Self {
            buckets: Vec::with_capacity(ID_SIZE * 8),
        }
    }
}

pub struct ClosestIter<'a, const ID_SIZE: usize>(&'a FlatRoutingTable<ID_SIZE>);

impl<'a, const ID_SIZE: usize> Iterator for ClosestIter<'a, ID_SIZE> {
    type Item = &'a Contact<ID_SIZE>;

    fn next(&mut self) -> Option<Self::Item> {
        todo!()
    }
}

pub struct Iter<'a, const ID_SIZE: usize>(&'a FlatRoutingTable<ID_SIZE>);

impl<'a, const ID_SIZE: usize> Iterator for Iter<'a, ID_SIZE> {
    type Item = &'a Contact<ID_SIZE>;

    fn next(&mut self) -> Option<Self::Item> {
        todo!()
    }
}

impl<'a, const ID_SIZE: usize> RoutingTable<'a, ID_SIZE> for FlatRoutingTable<ID_SIZE> {
    type ClosestIter = ClosestIter<'a, ID_SIZE>;
    type Iter = Iter<'a, ID_SIZE>;

    fn update(&mut self, _contact: Contact<ID_SIZE>) {
        todo!()
    }

    fn remove(&mut self, _id: &NodeId<ID_SIZE>) -> Option<Contact<ID_SIZE>> {
        todo!()
    }

    fn get(&self, _id: &NodeId<ID_SIZE>) -> Option<&Contact<ID_SIZE>> {
        todo!()
    }

    fn get_random(&self) -> Option<&Contact<ID_SIZE>> {
        todo!()
    }

    fn get_mut(&mut self, _id: &NodeId<ID_SIZE>) -> Option<&mut Contact<ID_SIZE>> {
        todo!()
    }

    fn get_closest_iter(&self, _id: &NodeId<ID_SIZE>) -> Self::ClosestIter {
        todo!()
    }

    fn contains(&self, _id: &NodeId<ID_SIZE>) -> bool {
        todo!()
    }

    fn contacts_iter(&self) -> Self::Iter {
        todo!()
    }
}
