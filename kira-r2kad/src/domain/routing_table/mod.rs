use derive_more::{Display, Error};
use std::ops::DerefMut;

use crate::domain::{Bucket, Contact, GroupingError, NodeId, ReplacementError, SharedPrefix};

pub mod flat_routing_table;
pub mod observable_routing_table;
#[cfg(test)]
pub mod single_bucket;
pub mod unlimited_pn_routing_table;

#[derive(Debug, Eq, PartialEq, Display, Error)]
pub enum AddError {
    #[display("Contact with id {_0} already exists")]
    AlreadyExists(#[error(not(source))] NodeId),
    #[display("Bucket is full, not added")]
    NotAdded,
}

#[derive(Debug, Eq, PartialEq, Display, Error)]
pub enum BucketSplitError {
    #[display("RoutingTable restricts splitting this bucket")]
    Unsplittable,
    #[display("Reached maximum number of buckets")]
    MaxBucketsReached,
}

#[derive(Debug, Eq, PartialEq, Display, Error)]
pub enum InsertionError {
    Add(AddError),
    BucketSplit(BucketSplitError),
}

impl From<AddError> for InsertionError {
    fn from(add_err: AddError) -> Self {
        Self::Add(add_err)
    }
}

impl From<BucketSplitError> for InsertionError {
    fn from(err: BucketSplitError) -> Self {
        Self::BucketSplit(err)
    }
}

/// A table managing [Contact]s.
///
/// # Buckets
///
/// A [RoutingTable] has to guarantee to have at least one [Bucket] at any given time.
///
/// # Underlay Neighbors
///
/// As some RoutingTable implementation may handle underlay neighbors in a different way
/// the caller has to be careful when using [RoutingTable::bucket] and [RoutingTable::bucket_mut].
/// In structures like
/// [UnlimitedPNRoutingTable](crate::domain::routing_table::unlimited_pn_routing_table::UnlimitedPNRoutingTable) the Neighbors
/// may not be included in the buckets.
///
/// As mostly accessing the buckets directly only happens if Insertion fails, this will ne problem.
pub trait RoutingTable<'a, const BUCKET_SIZE: usize> {
    /// Possible Write Guard for a mutable contact reference.
    ///
    /// Allows implementations to support RAII types to watch mutability of a contact.
    type ContactWriteGuard: DerefMut<Target = Contact>;
    /// Possible Write Guard for a mutable bucket reference.
    ///
    /// Allows implementations to support RAII types to watch mutability of a bucket.
    type BucketWriteGuard: DerefMut<Target = Bucket<BUCKET_SIZE>>;
    /// Iterator type over all [Contact]s.
    type Iter: Iterator<Item = &'a Contact>;
    /// Iterator type over mutable references of all [Contact]s.
    type IterMut: Iterator<Item = Self::ContactWriteGuard>;

    /// Iterator type over all [Bucket]s
    type BucketIter: Iterator<Item = &'a Bucket<BUCKET_SIZE>>;

    /// Returns the root [NodeId] of the [RoutingTable].
    fn root(&self) -> &NodeId;

    /// Returns the number of contacts inside the RoutingTable.
    fn len(&self) -> usize;

    /// Returns if the RoutingTable contains no Contacts.
    fn is_empty(&self) -> bool;

    /// Add a new [Contact] to the [RoutingTable].
    ///
    /// Returns an Error if the [Bucket] for the [Contact] is full or
    /// a [Contact] with the same [NodeId] is already present in the [Bucket].
    ///
    /// This doesn't perform any decision making if a bucket has to be split or another
    /// contact has to be replaced.
    /// This is entirely up to the caller.
    fn add(&mut self, contact: Contact) -> Result<(), AddError>;

    /// Removes an existing [Contact] and returns it if present.
    fn remove(&mut self, id: &NodeId) -> Option<Contact>;

    /// Replaces a [Contact] and returns the replaced one.
    fn replace(&mut self, id: &NodeId, with: Contact) -> Result<Contact, ReplacementError>;

    /// Returns an existing [Contact] if present.
    fn contact(&self, id: &NodeId) -> Option<&Contact>;

    /// Returns a random [Contact]s [NodeId] if the [RoutingTable] is not empty.
    fn random_id(&self) -> Option<&NodeId>;

    /// Returns a write guard to an existing [Contact] if present.
    fn contact_mut(&'a mut self, id: &NodeId) -> Option<Self::ContactWriteGuard>;

    /// Returns if a [Contact] with a given [NodeId] is present in the [RoutingTable].
    fn contains(&self, id: &NodeId) -> bool;

    /// Attempts to split the [Bucket] the id should be located in.
    /// The [Contact]s in the [Bucket] will be inserted in the appropriate [Bucket]s.
    ///
    /// This doesn't require a [Contact] inside the [Bucket] with the id.
    fn split_bucket(&mut self, id: &NodeId) -> Result<usize, BucketSplitError>;

    /// Returns the Bucket the [NodeId] should be located in based on the
    /// current state of the [RoutingTable].
    fn bucket(&self, of: &NodeId) -> &Bucket<BUCKET_SIZE>;

    /// Returns a mutable reference to the [Bucket] for the given [NodeId].
    fn bucket_mut(&'a mut self, of: &NodeId) -> Self::BucketWriteGuard;

    /// Returns the [Bucket] at the given index.
    fn bucket_by_index(&self, index: usize) -> &Bucket<BUCKET_SIZE>;

    /// Returns a mutable reference to the [Bucket] at the given index.
    fn bucket_by_index_mut(&'a mut self, index: usize) -> Self::BucketWriteGuard;

    /// Returns the index of the [Bucket] the given [NodeId] should be located in
    /// based on the current state of the [RoutingTable].
    fn get_bucket_index(&self, of: &NodeId) -> usize;

    /// Returns the fixed prefix length of the[Bucket] at the given index.
    fn get_bucket_prefix_length(&self, bucket_index: usize) -> usize;

    /// Inserts a [Contact] into the table by splitting the [Bucket] until
    /// Insertion succeeds or splitting failed.
    fn insert(&mut self, contact: Contact) -> Result<(), InsertionError> {
        match self.add(contact.clone()) {
            Ok(()) => Ok(()),
            Err(AddError::NotAdded) => {
                self.split_bucket(contact.id())?;
                self.insert(contact)
            }
            Err(err) => Err(InsertionError::Add(err)),
        }
    }

    /// Extends the [RoutingTable] with [Contact]s with an option to ignore errors.
    ///
    /// Will not return an [Error] if *drop_on_error* is *true*.
    fn extend<I: IntoIterator<Item = Contact>>(
        &mut self,
        drop_on_error: bool,
        iter: I,
    ) -> Result<(), InsertionError> {
        for contact in iter {
            // On Error: Either ignore or return
            match (self.insert(contact), drop_on_error) {
                (Ok(()), _) | (Err(_), true) => {}
                (Err(e), false) => return Err(e),
            }
        }

        Ok(())
    }

    /// Returns the **n** closest contacts sorted by distance to the given [NodeId].
    ///
    /// The returned pairs are the calculated [SharedPrefix] for every [Contact].
    /// These are also sorted from closest to farthest away.
    ///
    /// ## Improvements
    ///
    /// As this only returns a limited number of nodes an implementation based on iterators
    /// would be ideal in the future.
    fn closest(
        &self,
        to: &NodeId,
        n: usize,
        shared_prefix_grouping: usize,
    ) -> Result<Vec<(SharedPrefix, Contact)>, GroupingError>;

    /// Returns the next overlay hop to the given [NodeId]
    /// This method returns [None] if we are the closest overlay hop.
    ///
    /// This method checks the **n** closest neighbors as next hop candidates.
    ///
    /// The method is using **proximity routing** to determine the next overlay neighbor
    /// if there are multiple that would result in the same prefix progress.
    ///
    /// Using **n=1** disables proximity routing.
    fn next_hop(
        &self,
        to: &NodeId,
        n: usize,
        shared_prefix_grouping: usize,
    ) -> Result<Option<Contact>, GroupingError> {
        log::trace!(target: "routing_table", "Calculating next hop to {}", to);

        let closest = self.closest(to, n, shared_prefix_grouping)?;
        let (nearest_prefix, next_hop) = match closest.first() {
            None => {
                log::warn!(target: "routing_table","Node is isolated!");
                return Ok(None);
            } // table empty => we are the next hop
            Some((nearest_prefix, contact)) => (nearest_prefix, contact),
        };
        let root_prefix = self.root().shared_prefix_len(to, shared_prefix_grouping)?;

        // check if we are nearest
        if root_prefix.length > nearest_prefix.length {
            log::debug!(
                target: "routing_table",
                "Next hop is us since prefix of us is longer: {} > {}",
                root_prefix.length,
                nearest_prefix.length
            );
            return Ok(None);
        }

        // we can't make prefix progress or no proximity routing
        // uniquely select closest neighbor by XOR metric
        if nearest_prefix.length == root_prefix.length || n == 1 {
            log::debug!(
                target: "routing_table",
                "Unable to do prefix progress, selecting by XOR between us [{}] and nearest contact [{}]",
                root_prefix.xor,
                nearest_prefix.xor
            );
            return if root_prefix.xor < nearest_prefix.xor {
                Ok(None)
            } else {
                // assuming sorted list first is closest by XOR metric
                Ok(Some(next_hop.clone()))
            };
        }

        // check if lowest bucket
        // todo do this more efficiently
        let lowest = self.closest(self.root(), 1, shared_prefix_grouping)?;
        if lowest
            .first()
            .is_some_and(|(prefix, _)| prefix == nearest_prefix)
        {
            log::trace!(target: "routing_table", "Lowest bucket, select by XOR");
            // select closest by XOR
            return Ok(Some(next_hop.clone()));
        }

        // all contacts with the greatest prefix progress
        let next_hop = closest
            .iter()
            .take_while(|(prefix, _)| prefix.length == nearest_prefix.length)
            .map(|(_, contact)| contact)
            .min_by(|ca, cb| ca.path().size().cmp(&cb.path().size()));
        // todo select by xor if all path lengths (size) are the same

        Ok(next_hop.cloned())
        // todo test send to self if 1) isolated or 2) root closer than closest routing table entry
        // todo test if edge case lowest bucket (only XOR based) is covered
    }

    /// Iterator over all [Contact]s in the [RoutingTable].
    fn iter(&'a self) -> Self::Iter;

    /// Iterator over mutable references to all contacts in the [RoutingTable].
    fn iter_mut(&'a mut self) -> Self::IterMut;

    /// Iterator over all [Bucket]s in the [RoutingTable]
    fn bucket_iter(&'a self) -> Self::BucketIter;
}
