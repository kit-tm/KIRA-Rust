use std::error::Error;
use std::fmt::{Display, Formatter};
use std::ops::DerefMut;

use crate::domain::{Bucket, Contact, ContactState, GroupingError, NodeId, ReplacementError};

pub mod flat_routing_table;
pub mod observable_routing_table;
#[cfg(test)]
pub mod single_bucket;
pub mod unlimited_pn_routing_table;

#[derive(Debug, Eq, PartialEq)]
pub enum AddError {
    AlreadyExists(NodeId),
    NotAdded,
}

impl Display for AddError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAdded => write!(f, "Bucket is full, not added"),
            Self::AlreadyExists(id) => write!(f, "Contact with id {} already exists", id),
        }
    }
}

impl Error for AddError {}

#[derive(Debug, Eq, PartialEq)]
pub enum BucketSplitError {
    Unsplittable,
    MaxBucketsReached,
}

impl Display for BucketSplitError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsplittable => write!(f, "RoutingTable restricts splitting this bucket"),
            Self::MaxBucketsReached => write!(f, "Reached maximum number of buckets"),
        }
    }
}

impl Error for BucketSplitError {}

#[derive(Debug, Eq, PartialEq)]
pub enum InsertionError {
    Add(AddError),
    BucketSplit(BucketSplitError),
}

impl Display for InsertionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Add(err) => write!(f, "{}", err),
            Self::BucketSplit(err) => write!(f, "{}", err),
        }
    }
}

impl Error for InsertionError {}

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
/// # Physical Neighbors
///
/// As some RoutingTable implementation may handle physical neighbors in a different way
/// the caller has to be careful when using [RoutingTable::bucket] and [RoutingTable::bucket_mut].
/// In structures like [UnlimitedPNRoutingTable] the Neighbors may not be included
/// in the buckets.
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
    fn split_bucket(&mut self, id: &NodeId) -> Result<(), BucketSplitError>;

    /// Returns the Bucket the [NodeId] should be located in based on the
    /// current state of the [RoutingTable].
    fn bucket(&self, of: &NodeId) -> &Bucket<BUCKET_SIZE>;

    /// Returns a mutable reference to the [Bucket] for the given [NodeId].
    fn bucket_mut(&'a mut self, of: &NodeId) -> Self::BucketWriteGuard;

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

    fn get_closest(&self, to: &NodeId, shared_prefix_grouping: usize) -> Option<&Contact>;
}

/// Searches for the closest [Contact] to the given [NodeId] in the given [IntoIterator].
///
/// The closest [Contact] is determined in this deterministic way:
///
/// 1. Longer shared prefix wins
/// 2. If shared prefix length is the same: The smaller NodeId wins.
pub fn get_closest_in<'a, I: IntoIterator<Item = &'a Contact>>(
    iter: I,
    to: &NodeId,
    shared_prefix_grouping: usize,
) -> Result<Option<&'a Contact>, GroupingError> {
    let mut acc = None;

    for contact in iter {
        if contact.state() != &ContactState::Valid {
            continue;
        }

        let distance = to.shared_prefix_len(contact.id(), shared_prefix_grouping)?;

        if acc.is_none() {
            acc = Some((distance, contact));
            continue;
        }
        let (acc_prefix, acc_contact) = acc.as_ref().unwrap();

        // Closer to root means longer SharedPrefix Length
        if acc_prefix.length < distance.length {
            acc = Some((distance, contact));
            continue;
        }

        if acc_prefix.length == distance.length && contact.id() < acc_contact.id() {
            acc = Some((distance, contact));
            continue;
        }
    }

    Ok(acc.map(|(_, contact)| contact))
}

#[cfg(test)]
mod tests {
    use crate::domain::{Age, Contact, ContactState, NodeId, Path, StateSeqNr};

    #[test]
    fn get_closest_in() {
        let mut invalid_contact = Contact::new(
            Path::from([NodeId::with_msb(1)]),
            Age::from(1),
            StateSeqNr::from(0),
        );
        *invalid_contact.state_mut() = ContactState::Invalid;

        let contacts = vec![
            invalid_contact,
            Contact::new(
                Path::from([NodeId::with_msb(2)]),
                Age::from(1),
                StateSeqNr::from(0),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(3)]),
                Age::from(1),
                StateSeqNr::from(0),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(4)]),
                Age::from(1),
                StateSeqNr::from(0),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(5)]),
                Age::from(1),
                StateSeqNr::from(0),
            ),
        ];

        let closest = super::get_closest_in(&contacts, &NodeId::zero(), 1);
        assert!(closest.is_ok(), "Returned error: {:?}", closest);
        let closest = closest.unwrap();
        assert!(closest.is_some(), "Returned None");
        let closest = closest.unwrap();
        assert_eq!(closest, &contacts[1]);
    }
}
