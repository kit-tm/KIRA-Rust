use std::{
    cmp::Ordering,
    num::NonZeroU8,
    ops::DerefMut,
};

use derive_more::{
    Display,
    Error,
};
use tracing::Level;

#[doc(inline)]
pub use crate::domain::bucket::ReplacementError; // TODO: Create distinct RT-Error
use crate::domain::{
    Bucket,
    Contact,
    GroupingError,
    NodeId,
    SharedPrefix,
    hasher::Hasher,
};
pub mod flat_routing_table;
pub mod observable_routing_table;
#[cfg(test)]
pub mod single_bucket;
pub mod unlimited_uln_routing_table;

#[doc(inline)]
pub use flat_routing_table::FlatRoutingTable;
#[doc(inline)]
pub use observable_routing_table::ObservableRoutingTable;
#[cfg(test)]
#[doc(inline)]
pub use single_bucket::SingleBucketRT;
#[doc(inline)]
pub use unlimited_uln_routing_table::UnlimitedULNRoutingTable;

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
/// the caller has to be careful when using [RoutingTable::bucket].
/// In structures like [UnlimitedULNRoutingTable] the Neighbors may not be included in the buckets.
///
/// As mostly accessing the buckets directly only happens if Insertion fails, this will ne problem.
///
///[UnlimitedULNRoutingTable]: crate::domain::routing_table::unlimited_uln_routing_table::UnlimitedULNRoutingTable
pub trait RoutingTable<'a, const BUCKET_SIZE: usize> {
    /// Possible Write Guard for a mutable contact reference.
    ///
    /// Allows implementations to support RAII types to watch mutability of a contact.
    type ContactWriteGuard: DerefMut<Target = Contact>;

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

    /// Returns if a [Contact] with a given [NodeId] is present in the [RoutingTable] and contact fulfills given predicate
    fn contains_with<F>(&self, id: &NodeId, f: F) -> bool
    where
        F: Fn(&Contact) -> bool;

    /// Returns if a [Contact] with a given [NodeId] is among the closest contacts (either in deepest or second deepest bucket)
    fn is_close_contact(&self, id: &NodeId) -> bool;

    /// Attempts to split the [Bucket] the id should be located in.
    /// The [Contact]s in the [Bucket] will be inserted in the appropriate [Bucket]s.
    ///
    /// This doesn't require a [Contact] inside the [Bucket] with the id.
    fn split_bucket(&mut self, id: &NodeId) -> Result<usize, BucketSplitError>;

    /// Returns the Bucket the [NodeId] should be located in based on the
    /// current state of the [RoutingTable].
    fn bucket(&self, of: &NodeId) -> &Bucket<BUCKET_SIZE>;

    /// Returns the [Bucket] at the given index.
    fn bucket_by_index(&self, index: usize) -> &Bucket<BUCKET_SIZE>;

    /// Returns the index of the [Bucket] the given [NodeId] should be located in
    /// based on the current state of the [RoutingTable].
    fn get_bucket_index(&self, of: &NodeId) -> usize;

    /// Returns the fixed prefix length of the[Bucket] at the given index.
    fn get_bucket_prefix_length(&self, bucket_index: usize) -> u8;

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

    /// Returns contacts closest to the `target`.
    ///
    /// This method identifies at least **n** contacts that are closest to the target
    /// based on the [SharedPrefix][^xor]. It is specifically designed to support **Proximity
    /// Neighbor Selection (PNS)** by returning a superset of contacts when multiple
    /// candidates offer the same prefix progress.
    ///
    /// The returned pairs are the calculated [SharedPrefix] of every [Contact],
    /// sorted from closest to farthest.
    ///
    /// # Proximity Neighbor Selection
    ///
    /// When more than `n` contacts are returned, the caller can select the
    /// "best" `n` contacts among them based on physical proximity
    /// (e.g., shortest path)
    ///
    /// # Invariants
    ///
    /// * Only returns [_valid_] contacts.
    /// * Returns `≤n` contacts if the routing table contains fewer,
    ///   in which case all valid contacts are returned.
    ///
    /// ## Improvements
    ///
    /// As this only returns a limited number of nodes an implementation based on iterators
    /// would be ideal in the future.
    ///
    /// [_valid_]: crate::domain::contact::ContactState::Valid
    /// [^xor]: The contacts of the first bucket are strictly sorted by the XOR-metric
    ///       if the target is in the _last bucket_
    ///       (the bucket that contains the [`root`](RoutingTable::root),
    ///       which remains eligible for splitting).
    /// [^shared_prefix_length_select]:
    ///   The caller has to select nodes with longer shared prefix
    ///   length first.
    ///
    fn closest(
        &self,
        target: &NodeId,
        n: usize,
        shared_prefix_grouping: NonZeroU8,
    ) -> Result<Vec<(SharedPrefix, Contact)>, GroupingError>;

    /// Returns the next overlay hop to the given [NodeId]
    /// This method returns [None] if we are the closest overlay hop.
    ///
    /// This method checks the **n** closest neighbors as next hop candidates.
    ///
    /// The method is using **proximity routing** to determine the next overlay neighbor
    /// if there are multiple that would result in the same prefix progress.
    #[tracing::instrument(
        target = "routing_table",
        level=Level::DEBUG,
        skip(self)
        ret, err,
    )]
    fn next_hop(
        &self,
        target: &NodeId,
        shared_prefix_grouping: NonZeroU8,
    ) -> Result<Option<Contact>, GroupingError> {
        let closest = self.closest(target, 1, shared_prefix_grouping)?;
        let (nearest_prefix, nearest) = match closest.first() {
            None => {
                tracing::warn!(target: "routing_table", "Node is isolated!");
                return Ok(None);
            } // table empty => we are the next hop
            Some((nearest_prefix, contact)) => (nearest_prefix, contact),
        };
        let root_prefix = self
            .root()
            .shared_prefix_len(target, shared_prefix_grouping)?;

        // Check if root has longer shared prefix length.
        // Don't output next hop because root is closest.
        if root_prefix.length > nearest_prefix.length {
            tracing::trace!(
                target: "routing_table",
                reason = "Prefix progress of root greater than closest contact",
                root_pfx_len = ?root_prefix.length,
                nearest_pfx_len = ?nearest_prefix.length,
                %nearest,
                "Next hop is root",
            );
            return Ok(None);
        }

        // Strictly select by XOR-metric if no prefix progress can be made.
        if nearest_prefix.length == root_prefix.length {
            tracing::trace!(
                target: "routing_table",
                root_pfx_len = ?root_prefix.length,
                nearest_pfx_len = ?nearest_prefix.length,
                root_xor = %root_prefix.xor,
                nearest = %nearest,
                nearest_xor = %nearest_prefix.xor,
                reason = "No prefix progress of closest contact",
                "Selecting by XOR",
            );

            return if root_prefix.xor < nearest_prefix.xor {
                Ok(None)
            } else {
                // Closest doesn't sort contacts strictly by XOR-metric (except last bucket).
                // Only sorting by shared prefix length can be assumed.

                let nearest_prefix_len = nearest_prefix.bit_len();
                let next_hop = closest
                    .into_iter()
                    .take_while(|(prefix, _)| prefix.length == nearest_prefix_len)
                    .min_by(strict_xor_metric)
                    .map(|(_, c)| c);
                Ok(next_hop)
            };
        }

        // In the last bucket select strictly by XOR-metric.
        //
        // Ensured by closest: Only return requested amount (1) sorted by XOR-metric.
        // (Or a contacts with worse prefix).
        #[cfg(debug_assertions)]
        {
            let last_bucket_index = self.bucket(self.root());
            let bucket_index_of_nearest = self.bucket(nearest.id());

            if last_bucket_index == bucket_index_of_nearest
                && let Some((sp, _)) = closest.get(1)
            {
                debug_assert!(
                    nearest_prefix.bit_len() > sp.bit_len(),
                    "No alternative contacts with equal shared prefix length in last bucket; strictly select by XOR-metric"
                );
            }
        }

        // Proximity Neighbor Selection:
        // If multiple (valid) contacts with the same shared prefix
        // exist, select by contact's path length.
        let nearest_prefix_len = nearest_prefix.bit_len();
        let next_hop = closest
            .into_iter()
            .take_while(|(prefix, _)| prefix.bit_len() == nearest_prefix_len)
            .min_by(proximity_neighbor_selection)
            .map(|(_, c)| c);

        return Ok(next_hop);

        #[tracing::instrument(level = Level::TRACE, ret)]
        fn strict_xor_metric(
            (spa, _): &(SharedPrefix, Contact),
            (spb, _): &(SharedPrefix, Contact),
        ) -> Ordering {
            debug_assert_eq!(
                spa.bit_len(),
                spb.bit_len(),
                "Select contacts with most prefix progress first"
            );
            spa.xor().cmp(spb.xor())
        }

        #[tracing::instrument(level = Level::TRACE, ret)]
        fn proximity_neighbor_selection(
            (spa, ca): &(SharedPrefix, Contact),
            (spb, cb): &(SharedPrefix, Contact),
        ) -> Ordering {
            debug_assert_eq!(
                spa.bit_len(),
                spb.bit_len(),
                "Select contacts with most prefix progress first"
            );

            // TODO what if paths are not set and unwrap() may fail?
            match ca.path().unwrap().size().cmp(&cb.path().unwrap().size()) {
                // Tie breaker: XOR-Metric
                Ordering::Equal => spa.xor().cmp(spb.xor()),
                ord => ord,
            }
        }
    }

    /// Iterator over all [Contact]s in the [RoutingTable].
    fn iter(&self) -> impl Iterator<Item = &Contact>;

    /// Iterator over mutable references to all contacts in the [RoutingTable].
    fn iter_mut(&'a mut self) -> impl Iterator<Item = Self::ContactWriteGuard>;

    /// Iterator over all [Bucket]s in the [RoutingTable]
    fn bucket_iter(&'a self) -> Self::BucketIter;

    // a hash function used to hash paths into PathIDs
    fn path_hasher(&self) -> Hasher;
}

type PrefixContact = (SharedPrefix, Contact);

fn sorter_xor((a, _): &PrefixContact, (b, _): &PrefixContact) -> Ordering {
    a.cmp(b) // compares by shared prefix with xor as tie-breaker
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        Path,
        SafeStateSeqNr,
    };

    fn test_next_hop_isolated_impl<RT>(table_factory: impl FnOnce(NodeId) -> RT)
    where
        for<'a> RT: RoutingTable<'a, 4>,
    {
        crate::tests::init();
        let root = NodeId::ZERO;
        let target = NodeId::MAX;
        let table = table_factory(root);
        let res = table.next_hop(&target, NonZeroU8::new(1).unwrap()).unwrap();
        assert!(res.is_none());
    }

    #[test]
    fn test_next_hop_isolated_flat_routing_table() {
        test_next_hop_isolated_impl(|root| FlatRoutingTable::<4, 1>::new(root).unwrap());
    }

    #[test]
    fn test_next_hop_isolated_single_bucket_rt() {
        test_next_hop_isolated_impl(SingleBucketRT::<4>::new);
    }

    #[test]
    fn test_next_hop_isolated_unlimited_uln_routing_table() {
        test_next_hop_isolated_impl(|root| {
            UnlimitedULNRoutingTable::from(FlatRoutingTable::<4, 1>::new(root).unwrap())
        });
    }

    fn test_next_hop_root_closer_impl<RT>(table_factory: impl FnOnce(NodeId) -> RT)
    where
        for<'a> RT: RoutingTable<'a, 4>,
    {
        crate::tests::init();
        let root = NodeId::with_msb(0xff); // closer to target NodeId::MAX than NodeId::ZERO
        let mut table = table_factory(root);
        let target = NodeId::MAX;

        let contact = Contact::new(
            Path::from(NodeId::ZERO),
            SafeStateSeqNr::try_from(1).unwrap(),
        );
        table.add(contact).unwrap();

        let res = table.next_hop(&target, NonZeroU8::new(1).unwrap()).unwrap();
        assert!(res.is_none(), "Root is closer than contact");
    }

    #[test]
    fn test_next_hop_root_closer_flat_routing_table() {
        test_next_hop_root_closer_impl(|root| FlatRoutingTable::<4, 1>::new(root).unwrap());
    }

    #[test]
    fn test_next_hop_root_closer_single_bucket_rt() {
        test_next_hop_root_closer_impl(SingleBucketRT::<4>::new);
    }

    #[test]
    fn test_next_hop_root_closer_unlimited_uln_routing_table() {
        test_next_hop_root_closer_impl(|root| {
            UnlimitedULNRoutingTable::from(FlatRoutingTable::<4, 1>::new(root).unwrap())
        });
    }

    fn test_next_hop_xor_tiebreaker_impl<RT>(table_factory: impl FnOnce(NodeId) -> RT)
    where
        for<'a> RT: RoutingTable<'a, 4>,
    {
        crate::tests::init();
        let root = NodeId::with_msb(0b1111_0000);
        let mut table = table_factory(root);

        let c_id = NodeId::with_msb(0b1111_0001);
        let c = Contact::new(Path::from(c_id), SafeStateSeqNr::try_from(1).unwrap());
        table.add(c.clone()).unwrap();

        // Case 1: Contact XOR is smaller
        let target = NodeId::MAX;
        let res = table.next_hop(&target, NonZeroU8::new(1).unwrap()).unwrap();
        assert_eq!(
            res.unwrap().id(),
            c.id(),
            "Contact XOR is smaller, should return contact"
        );

        // Case 2: Root XOR is smaller
        let target = NodeId::MAX ^ NodeId::with_msb(0x0f);
        let res = table.next_hop(&target, NonZeroU8::new(1).unwrap()).unwrap();
        assert!(res.is_none(), "Root XOR is smaller, should return None");
    }

    #[test]
    fn test_next_hop_xor_tiebreaker_flat_routing_table() {
        test_next_hop_xor_tiebreaker_impl(|root| FlatRoutingTable::<4, 1>::new(root).unwrap());
    }

    #[test]
    fn test_next_hop_xor_tiebreaker_single_bucket_rt() {
        test_next_hop_xor_tiebreaker_impl(SingleBucketRT::<4>::new);
    }

    #[test]
    fn test_next_hop_xor_tiebreaker_unlimited_uln_routing_table() {
        test_next_hop_xor_tiebreaker_impl(|root| {
            UnlimitedULNRoutingTable::from(FlatRoutingTable::<4, 1>::new(root).unwrap())
        });
    }
}
