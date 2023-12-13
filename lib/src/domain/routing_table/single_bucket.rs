use std::cmp::Ordering;

use rand::Rng;

use crate::domain::observable_routing_table::NonObservableRoutingTable;
use crate::domain::{
    AddError, Bucket, BucketInsertionError, BucketSplitError, Contact, GroupingError, NodeId,
    ReplacementError, RoutingTable, SharedPrefix,
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
    type BucketWriteGuard = &'a mut Bucket<BUCKET_SIZE>;
    type Iter = crate::domain::bucket::Iter<&'a Contact>;
    type IterMut = crate::domain::bucket::Iter<&'a mut Contact>;

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
        let random = rand::thread_rng().gen_range(0..len);
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
    fn split_bucket(&mut self, _id: &NodeId) -> Result<(), BucketSplitError> {
        Err(BucketSplitError::MaxBucketsReached)
    }

    /// Returns a reference to the only [Bucket] in this [RoutingTable].
    fn bucket(&self, _of: &NodeId) -> &Bucket<BUCKET_SIZE> {
        &self.bucket
    }

    /// Returns a mutable reference to the only [Bucket] in this [RoutingTable].
    fn bucket_mut(&'a mut self, _of: &NodeId) -> Self::BucketWriteGuard {
        &mut self.bucket
    }

    fn closest(
        &self,
        to: &NodeId,
        n: usize,
        shared_prefix_grouping: usize,
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

    fn iter(&'a self) -> Self::Iter {
        (&self.bucket).into_iter()
    }

    fn iter_mut(&'a mut self) -> Self::IterMut {
        (&mut self.bucket).into_iter()
    }

    fn is_in_last_two_buckets(&self, node_id: &NodeId) -> bool {
        true
    }

    fn range_last_two_buckets(&self) -> (NodeId, NodeId) {
        (NodeId::zero(),NodeId::max_value())
    }
}

impl<'a, const BUCKET_SIZE: usize> NonObservableRoutingTable<'a, BUCKET_SIZE>
    for SingleBucketRT<BUCKET_SIZE>
{
}

impl<'a, const BUCKET_SIZE: usize> IntoIterator for &'a SingleBucketRT<BUCKET_SIZE> {
    type Item = &'a Contact;
    type IntoIter = crate::domain::bucket::Iter<&'a Contact>;

    fn into_iter(self) -> Self::IntoIter {
        (&self.bucket).into_iter()
    }
}

impl<'a, const BUCKET_SIZE: usize> IntoIterator for &'a mut SingleBucketRT<BUCKET_SIZE> {
    type Item = &'a mut Contact;
    type IntoIter = crate::domain::bucket::Iter<&'a mut Contact>;

    fn into_iter(self) -> Self::IntoIter {
        (&mut self.bucket).into_iter()
    }
}
