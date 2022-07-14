use rand::Rng;
use crate::domain::observable_routing_table::NonObservableRoutingTable;
use crate::domain::{AddError, Bucket, BucketInsertionError, BucketSplitError, Contact, NodeId, ReplacementError, RoutingTable};

/// A [RoutingTable] with a single not splittable [Bucket].
///
/// Mainly for testing purposes.
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
            Ok(()) => Ok(())
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
}

impl<'a, const BUCKET_SIZE: usize> NonObservableRoutingTable<'a, BUCKET_SIZE>
    for SingleBucketRT<BUCKET_SIZE>
{
}
