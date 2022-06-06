use crate::domain::{Bucket, Contact, GroupingError, NodeId, RoutingTable, SharedPrefix};

/// A [RoutingTable] implemented as flat array of [Bucket]s.
///
/// This [RoutingTable] has a root [NodeId] which the distance is computed to.
#[derive(Debug)]
pub struct FlatRoutingTable<const ID_SIZE: usize, const ACC: usize = 1> {
    buckets: Vec<Bucket<ID_SIZE>>,
    root: NodeId<ID_SIZE>,
}

impl<const ID_SIZE: usize, const ACC: usize> FlatRoutingTable<ID_SIZE, ACC> {
    /// Creates a [RoutingTable] with 0 capacity.
    pub const fn new(root: NodeId<ID_SIZE>) -> Result<Self, GroupingError> {
        Self::with_buckets(root, Vec::new())
    }

    /// Create a [RoutingTable] with the capacity of its maximum possible number of buckets.
    ///
    /// That is equal to the [NodeId] Size in Bits.
    pub fn with_full_capacity(root: NodeId<ID_SIZE>) -> Result<Self, GroupingError> {
        Self::with_buckets(root, Vec::with_capacity(ID_SIZE * 8))
    }

    fn with_buckets(
        root: NodeId<ID_SIZE>,
        buckets: Vec<Bucket<ID_SIZE>>,
    ) -> Result<Self, GroupingError> {
        if ACC < ID_SIZE {
            return Err(GroupingError::Invalid {
                id_size: ID_SIZE,
                group_size: ACC,
            });
        }
        Ok(Self { buckets, root })
    }

    /// Returns the index of the Bucket
    fn get_bucket_index(&self, of: &NodeId<ID_SIZE>) -> usize {
        let SharedPrefix {
            xor,
            value: prefix_len,
        } = self
            .root
            .shared_prefix_len(of, ACC)
            .expect("GroupingError after checking");

        // bitindex is now the index of the LSB of the first non-zero digit in delta
        let bitindex = ID_SIZE - (prefix_len + 1) * ACC;
        // This is my key
        if bitindex < 0 {
            return self.buckets.len() - 1;
        }

        // bitindex is the LSB of the first non-zero digit, so digit must not be zero
        assert_ne!(xor.bits(bitindex..ACC), Ok(0));

        todo!()
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
