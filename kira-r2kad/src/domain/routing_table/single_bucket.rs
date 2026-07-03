use std::num::NonZeroU8;

use rand::Rng;
use tracing::Level;

use crate::domain::ContactState;
use crate::domain::observable_routing_table::NonObservableRoutingTable;
use crate::domain::routing_table::sorter_xor;
use crate::domain::{
    AddError, Bucket, BucketInsertionError, BucketSplitError, Contact, GroupingError, NodeId,
    NotViaStateList, ReplacementError, RoutingTable, SharedPrefix, hasher::Hasher,
};

/// A [RoutingTable] with a single not splittable [Bucket].
///
/// Mainly for testing purposes.
#[derive(Debug)]
pub struct SingleBucketRT<const BUCKET_SIZE: usize> {
    root_id: NodeId,
    bucket: Bucket<BUCKET_SIZE>,
}

impl<const BUCKET_SIZE: usize> SingleBucketRT<BUCKET_SIZE> {
    /// Create a new [SingleBucketRT] with an empty [Bucket].
    pub fn new(root_id: NodeId) -> Self {
        Self {
            root_id,
            bucket: Bucket::new(),
        }
    }

    /// Create a new [SingleBucketRT] with the given [Bucket].
    pub fn with_bucket(root_id: NodeId, bucket: Bucket<BUCKET_SIZE>) -> Self {
        Self { root_id, bucket }
    }
}

impl<'a, const BUCKET_SIZE: usize> RoutingTable<'a, BUCKET_SIZE> for SingleBucketRT<BUCKET_SIZE> {
    type ContactWriteGuard = &'a mut Contact;
    type BucketIter = std::iter::Once<&'a Bucket<BUCKET_SIZE>>;

    fn root(&self) -> &NodeId {
        &self.root_id
    }

    fn len(&self) -> usize {
        1
    }

    fn is_empty(&self) -> bool {
        false
    }

    fn add(&mut self, contact: Contact) -> Result<(), AddError> {
        match self.bucket.insert(contact) {
            Err(BucketInsertionError::DuplicateId(id)) => Err(AddError::AlreadyExists(id)),
            Err(BucketInsertionError::Full) => Err(AddError::NotAdded),
            Ok(()) => Ok(()),
        }
    }

    fn remove(&mut self, id: &NodeId) -> Option<Contact> {
        self.bucket.remove(id)
    }

    fn replace(&mut self, id: &NodeId, with: Contact) -> Result<Contact, ReplacementError> {
        self.bucket.replace(id, with)
    }

    fn contact(&self, id: &NodeId) -> Option<&Contact> {
        self.bucket.get(id)
    }

    fn random_id(&self) -> Option<&NodeId> {
        let len = self.bucket.len();
        let random = rand::rng().random_range(0..len);
        let mut iter = self.bucket.iter();
        iter.nth(random).map(|contact| contact.id())
    }

    fn contact_mut(&'a mut self, id: &NodeId) -> Option<Self::ContactWriteGuard> {
        self.bucket.get_mut(id)
    }

    fn contains(&self, id: &NodeId) -> bool {
        self.bucket.contains(id)
    }

    fn contains_with<F>(&self, id: &NodeId, f: F) -> bool
    where
        F: Fn(&Contact) -> bool,
    {
        if let Some(c) = self.bucket.get(id) {
            f(c)
        } else {
            false
        }
    }

    fn is_close_contact(&self, _id: &NodeId) -> bool {
        true
    }

    /// Emits an [BucketSplitError::MaxBucketsReached] every time.
    fn split_bucket(&mut self, _id: &NodeId) -> Result<usize, BucketSplitError> {
        Err(BucketSplitError::MaxBucketsReached)
    }

    /// Returns a reference to the only [Bucket] in this [RoutingTable].
    fn bucket(&self, _of: &NodeId) -> &Bucket<BUCKET_SIZE> {
        &self.bucket
    }

    #[tracing::instrument(
        level = Level::TRACE,
        target = "routing_table::single_bucket",
        ret, err
    )]
    fn closest(
        &self,
        target: &NodeId,
        n: usize,
        shared_prefix_grouping: NonZeroU8,
    ) -> Result<Vec<(SharedPrefix, Contact)>, GroupingError> {
        // check for valid grouping first because we don't want unexpectedly panic inside iters
        target.shared_prefix_len(&self.root_id, shared_prefix_grouping)?;

        let mut result: Vec<_> = self
            .bucket
            .iter_valid_with_prefix(target, shared_prefix_grouping)
            .collect();

        // always strictly by XOR-metric because last bucket
        result.sort_unstable_by(sorter_xor);
        result.truncate(n); // no additional contacts

        Ok(result)
    }

    fn iter(&self) -> impl Iterator<Item = &Contact> {
        (&self.bucket).into_iter()
    }

    fn iter_mut(&'a mut self) -> impl Iterator<Item = Self::ContactWriteGuard> {
        (&mut self.bucket).into_iter()
    }

    fn bucket_iter(&'a self) -> Self::BucketIter {
        std::iter::once(&self.bucket)
    }

    fn bucket_by_index(&self, _: usize) -> &Bucket<BUCKET_SIZE> {
        &self.bucket
    }

    fn get_bucket_index(&self, _: &NodeId) -> usize {
        0
    }

    fn get_bucket_prefix_length(&self, _: usize) -> u8 {
        0
    }

    fn path_hasher(&self) -> Hasher {
        Hasher::default()
    }
}

impl<const BUCKET_SIZE: usize> NonObservableRoutingTable<'_, BUCKET_SIZE>
    for SingleBucketRT<BUCKET_SIZE>
{
}

impl<'a, const BUCKET_SIZE: usize> IntoIterator for &'a SingleBucketRT<BUCKET_SIZE> {
    type Item = &'a Contact;
    type IntoIter = crate::domain::bucket::IntoIter<&'a Contact>;

    fn into_iter(self) -> Self::IntoIter {
        (&self.bucket).into_iter()
    }
}

impl<'a, const BUCKET_SIZE: usize> IntoIterator for &'a mut SingleBucketRT<BUCKET_SIZE> {
    type Item = &'a mut Contact;
    type IntoIter = crate::domain::bucket::IntoIter<&'a mut Contact>;

    fn into_iter(self) -> Self::IntoIter {
        (&mut self.bucket).into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    use crate::domain::{Path, SafeStateSeqNr};

    #[test]
    fn test_closest_single() -> Result<(), Box<dyn Error>> {
        crate::tests::init();
        let root = NodeId::ZERO;
        let mut table = SingleBucketRT::<4>::new(root);

        let ids = [
            NodeId::with_msb(0b0100_0000), // 0x4
            NodeId::with_msb(0b0010_0000), // 0x2
            NodeId::with_msb(0b1000_0000), // 0x8
            NodeId::with_msb(0b1100_0000), // 0xc
        ];

        for id in &ids {
            table.insert(Contact::new(
                Path::from(*id),
                SafeStateSeqNr::try_from(1).unwrap(),
            ))?;
        }

        let query_id = NodeId::with_msb(0b1110_0000);
        let results: Vec<_> = table
            .closest(&query_id, 2, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");

        // Closest to 1110... should be 1100... then 1000...
        // 1110... ^ 1100... = 0010... (prefix len 2)
        // 1110... ^ 1000... = 0110... (prefix len 1)
        // 1110... ^ 0100... = 1010... (prefix len 0)
        // 1110... ^ 0010... = 1100... (prefix len 0)

        assert_eq!(results.len(), 2);

        // 1100...
        assert_eq!(results[0].1, ids[3]);
        assert_eq!(results[0].0.bit_len(), 2);
        // 1000...
        assert_eq!(results[1].1, ids[2]);
        assert_eq!(results[1].0.bit_len(), 1);

        Ok(())
    }

    #[test]
    fn test_closest_invalid_contacts() -> Result<(), Box<dyn Error>> {
        crate::tests::init();
        let root = NodeId::ZERO;
        let mut table = SingleBucketRT::<4>::new(root);

        let ids = [
            NodeId::with_msb(0b1000_0000), // 0x8 , bucket 1...
            NodeId::with_msb(0b1100_0000), // 0xc , bucket 1...
            NodeId::with_msb(0b0100_0000), // 0x4 , bucket 0...
            NodeId::with_msb(0b0010_0000), // 0x2 , bucket 0...
        ];

        for id in &ids {
            table.insert(Contact::new(
                Path::from(*id),
                SafeStateSeqNr::try_from(1).unwrap(),
            ))?;
        }

        *table.contact_mut(&ids[1]).unwrap().state_mut() =
            ContactState::Invalid(NotViaStateList::default());
        assert_eq!(
            table.contact(&ids[1]).unwrap().state(),
            &ContactState::Invalid(NotViaStateList::default()),
            "1100... is invalid",
        );

        let query_id = NodeId::with_msb(0b1100_0001);
        // Bucket 1... consists of the two closest entries 1100... and 1000...
        // but shouldn't return 1100 because it is invalid
        let results: Vec<_> = table
            .closest(&query_id, 1, NonZeroU8::new(1).unwrap())?
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");

        assert_eq!(results.len(), 1);

        assert_eq!(results[0].1, ids[0]);
        assert_eq!(results[0].0.bit_len(), 1);

        Ok(())
    }
}
