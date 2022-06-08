use std::error::Error;
use std::fmt::{Display, Formatter};

use crate::domain::{Contact, NodeId, ReplacementError};

#[derive(Debug, Eq, PartialEq)]
pub enum AddError<const ID_SIZE: usize> {
    AlreadyExists(NodeId<ID_SIZE>),
    NotAdded,
}

impl<const ID_SIZE: usize> Display for AddError<ID_SIZE> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAdded => write!(f, "Bucket is full, not added"),
            Self::AlreadyExists(id) => write!(f, "Contact with id {} already exists", id),
        }
    }
}

impl<const ID_SIZE: usize> Error for AddError<ID_SIZE> {}

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
pub enum InsertionError<const ID_SIZE: usize> {
    Add(AddError<ID_SIZE>),
    BucketSplit(BucketSplitError),
}

impl<const ID_SIZE: usize> Display for InsertionError<ID_SIZE> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Add(err) => write!(f, "{}", err),
            Self::BucketSplit(err) => write!(f, "{}", err),
        }
    }
}

impl<const ID_SIZE: usize> Error for InsertionError<ID_SIZE> {}

impl<const ID_SIZE: usize> From<AddError<ID_SIZE>> for InsertionError<ID_SIZE> {
    fn from(add_err: AddError<ID_SIZE>) -> Self {
        Self::Add(add_err)
    }
}

impl<const ID_SIZE: usize> From<BucketSplitError> for InsertionError<ID_SIZE> {
    fn from(err: BucketSplitError) -> Self {
        Self::BucketSplit(err)
    }
}

pub trait RoutingTable<'a, const ID_SIZE: usize> {
    type Iter: Iterator<Item = &'a Contact<ID_SIZE>>;

    /// Add a new [Contact] to the [RoutingTable].
    ///
    /// Returns an Error if the [Bucket] for the [Contact] is full or
    /// a [Contact] with the same [NodeId] is already present in the [Bucket].
    ///
    /// This doesn't perform any decision making if a bucket has to be split or another
    /// contact has to be replaced.
    /// This is entirely up to the caller.
    fn add(&mut self, contact: Contact<ID_SIZE>) -> Result<(), AddError<ID_SIZE>>;

    /// Removes an existing [Contact] and returns it if present.
    fn remove(&mut self, id: &NodeId<ID_SIZE>) -> Option<Contact<ID_SIZE>>;

    /// Replaces a [Contact] and returns the replaced one.
    fn replace(
        &mut self,
        id: &NodeId<ID_SIZE>,
        with: Contact<ID_SIZE>,
    ) -> Result<(), ReplacementError<ID_SIZE>>;

    /// Returns an existing [Contact] if present.
    fn contact(&self, id: &NodeId<ID_SIZE>) -> Option<&Contact<ID_SIZE>>;

    /// Returns a random [Contact]s [NodeId] if the [RoutingTable] is not empty.
    fn random_id(&self) -> Option<&NodeId<ID_SIZE>>;

    /// Returns a mutable reference to an existing [Contact] if present.
    fn contact_mut(&mut self, id: &NodeId<ID_SIZE>) -> Option<&mut Contact<ID_SIZE>>;

    /// Returns if a [Contact] with a given [NodeId] is present in the [RoutingTable].
    fn contains(&self, id: &NodeId<ID_SIZE>) -> bool;

    /// Returns an [Iterator] over all [Contact]s in this [RoutingTable].
    fn contacts_iter(&'a self) -> Self::Iter;

    /// Attempts to split the [Bucket] the id should be located in.
    /// The [Contact]s in the [Bucket] will be inserted in the appropriate [Bucket]s.
    ///
    /// This doesn't require a [Contact] inside the [Bucket] with the id.
    fn split_bucket(&mut self, id: &NodeId<ID_SIZE>) -> Result<(), BucketSplitError>;

    /// Inserts a [Contact] into the table by splitting the [Bucket] until
    /// Insertion succeeds or splitting failed.
    fn insert(&mut self, contact: Contact<ID_SIZE>) -> Result<(), InsertionError<ID_SIZE>> {
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
    fn extend<I: IntoIterator<Item = Contact<ID_SIZE>>>(
        &mut self,
        drop_on_error: bool,
        iter: I,
    ) -> Result<(), InsertionError<ID_SIZE>> {
        for contact in iter {
            // On Error: Either ignore or return
            match (self.insert(contact), drop_on_error) {
                (Ok(()), _) | (Err(_), true) => {}
                (Err(e), false) => return Err(e),
            }
        }

        Ok(())
    }
}
