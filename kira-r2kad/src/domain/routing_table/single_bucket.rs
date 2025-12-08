// because we mock a lot here
#![allow(unused_variables)]

use std::cmp::Ordering;
use std::num::NonZeroU8;

use rand::Rng;

use crate::domain::observable_routing_table::NonObservableRoutingTable;
use crate::domain::{
    AddError, Bucket, BucketInsertionError, BucketSplitError, Contact, GroupingError, Hasher,
    NodeId, ReplacementError, RoutingTable, SharedPrefix,
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

    /// Emits an [BucketSplitError::MaxBucketsReached] every time.
    fn split_bucket(&mut self, _id: &NodeId) -> Result<usize, BucketSplitError> {
        Err(BucketSplitError::MaxBucketsReached)
    }

    /// Returns a reference to the only [Bucket] in this [RoutingTable].
    fn bucket(&self, _of: &NodeId) -> &Bucket<BUCKET_SIZE> {
        &self.bucket
    }

    fn closest(
        &self,
        to: &NodeId,
        n: usize,
        shared_prefix_grouping: NonZeroU8,
    ) -> Result<Vec<(SharedPrefix, Contact)>, GroupingError> {
        let mut result = Vec::with_capacity(n);
        for contact in self.bucket.iter().take(n) {
            let prefix = to.shared_prefix_len(contact.id(), shared_prefix_grouping)?;
            result.push((prefix, contact.clone()));
        }
        result.sort_by(
            |first: &(SharedPrefix, Contact), second: &(SharedPrefix, Contact)| {
                if first.0 < second.0 {
                    return Ordering::Less;
                }

                if first.0 == second.0 && first.1.id() < second.1.id() {
                    return Ordering::Less;
                }

                Ordering::Greater
            },
        );
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

    fn bucket_by_index(&self, index: usize) -> &Bucket<BUCKET_SIZE> {
        &self.bucket
    }

    fn get_bucket_index(&self, of: &NodeId) -> usize {
        0
    }

    fn get_bucket_prefix_length(&self, bucket_index: usize) -> u8 {
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
