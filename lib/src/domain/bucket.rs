use std::error::Error;
use std::fmt::{Display, Formatter};

use crate::domain::{Contact, NodeId};

pub const DEFAULT_BUCKET_SIZE: usize = 20;

#[derive(Debug)]
pub enum InsertionError<const ID_SIZE: usize> {
    DuplicateId(NodeId<ID_SIZE>),
    Full,
}

impl<const ID_SIZE: usize> Display for InsertionError<ID_SIZE> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full => write!(f, "Tried inserting into full bucket"),
            Self::DuplicateId(id) => write!(f, "Contact with id {} already in bucket", id),
        }
    }
}

impl<const ID_SIZE: usize> Error for InsertionError<ID_SIZE> {}

/// A [Bucket] with fixed size used in the [RoutingTable].
///
/// The current implementation is backed by a [Vec] which can make problems
/// with memory locality.
/// TODO: Check if this is a performance overhead
#[derive(Debug)]
pub struct Bucket<const ID_SIZE: usize> {
    inner: Vec<Contact<ID_SIZE>>,
}

impl<const ID_SIZE: usize> Default for Bucket<ID_SIZE> {
    fn default() -> Self {
        Bucket::new()
    }
}

impl<const ID_SIZE: usize> Bucket<ID_SIZE> {
    /// Create a new [Bucket] with size of [DEFAULT_BUCKET_SIZE].
    pub fn new() -> Self {
        Self {
            inner: Vec::with_capacity(DEFAULT_BUCKET_SIZE),
        }
    }

    /// Create a new [Bucket] with a given size.
    pub fn with_size<const SIZE: usize>() -> Self {
        Self {
            inner: Vec::with_capacity(SIZE),
        }
    }

    /// Returns if the [Bucket] contains any [Contact].
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns if the [Bucket] has reached it's maximum size.
    pub fn is_full(&self) -> bool {
        self.inner.len() == self.inner.capacity()
    }

    /// Gets a [Contact] in the [Bucket] by [NodeId].
    pub fn get(&self, id: &NodeId<ID_SIZE>) -> Option<&Contact<ID_SIZE>> {
        self.inner.iter().find(|contact| contact.id() == id)
    }

    /// Returns the number of [Contact]s in the [Bucket].
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns the maximum size of the [Bucket].
    pub fn size(&self) -> usize {
        self.inner.capacity()
    }

    /// Returns if a [Contact] with a given [NodeId] is in the [Bucket].
    pub fn contains(&self, id: &NodeId<ID_SIZE>) -> bool {
        self.inner.iter().any(|contact| contact.id() == id)
    }

    /// Returns a mutable reference to the place of the [Contact] with the given [NodeId]
    /// if present in the [Bucket].
    fn get_mut(&mut self, id: &NodeId<ID_SIZE>) -> Option<&mut Contact<ID_SIZE>> {
        self.inner.iter_mut().find(|contact| contact.id() == id)
    }

    /// Tries to insert a [Contact] into the [Bucket].
    pub fn insert(&mut self, contact: Contact<ID_SIZE>) -> Result<(), InsertionError<ID_SIZE>> {
        if self.contains(contact.id()) {
            return Err(InsertionError::DuplicateId(contact.into_id()));
        }

        if self.is_full() {
            return Err(InsertionError::Full);
        }

        self.inner.push(contact);
        Ok(())
    }

    /// Replaces a [Contact] with the given [NodeId] with another [Contact].
    /// Returns if the Replacement was successful.
    pub fn replace(&mut self, replace_id: &NodeId<ID_SIZE>, with: Contact<ID_SIZE>) -> bool {
        if let Some(place) = self.get_mut(replace_id) {
            *place = with;
            return true;
        }

        false
    }

    /// Removes a [Contact] from the [Bucket] returning it if present.
    pub fn remove(&mut self, id: &NodeId<ID_SIZE>) -> Option<Contact<ID_SIZE>> {
        if let Some(index) = self.inner.iter().position(|contact| contact.id() == id) {
            return Some(self.inner.remove(index));
        }

        None
    }
}

impl<'a, const ID_SIZE: usize> IntoIterator for &'a Bucket<ID_SIZE> {
    type Item = &'a Contact<ID_SIZE>;
    type IntoIter = Iter<&'a Contact<ID_SIZE>>;

    fn into_iter(self) -> Self::IntoIter {
        Iter(self.inner.iter().rev().collect())
    }
}

impl<const ID_SIZE: usize> IntoIterator for Bucket<ID_SIZE> {
    type Item = Contact<ID_SIZE>;
    type IntoIter = Iter<Contact<ID_SIZE>>;

    fn into_iter(self) -> Self::IntoIter {
        let mut inner = self.inner;
        inner.reverse();
        Iter(inner)
    }
}

pub struct Iter<I>(Vec<I>);

impl<I> Iterator for Iter<I> {
    type Item = I;

    fn next(&mut self) -> Option<Self::Item> {
        self.0.pop()
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::{Age, Bucket, Contact, NodeId, Path, StateSeqNr};

    #[test]
    fn smoke_test() {
        let mut bucket = Bucket::with_size::<2>();

        assert!(bucket.is_empty());
        assert!(!bucket.is_full());

        let contact = Contact::new(
            NodeId::from([0, 1]),
            Age::from(0),
            Path::empty(),
            StateSeqNr::from(0),
        );

        assert!(bucket.insert(contact.clone()).is_ok());

        assert!(!bucket.is_empty());
        assert!(!bucket.is_full());
        assert!(bucket.contains(contact.id()));
        assert_eq!(bucket.get(contact.id()), Some(&contact));

        assert!(bucket.insert(contact.clone()).is_err());

        let second_contact = Contact::new(
            NodeId::from([0, 2]),
            Age::from(0),
            Path::empty(),
            StateSeqNr::from(0),
        );

        assert!(bucket.insert(second_contact.clone()).is_ok());

        assert!(!bucket.is_empty());
        assert!(bucket.is_full());
        assert!(bucket.contains(contact.id()));
        assert!(bucket.contains(second_contact.id()));
        assert_eq!(bucket.get(contact.id()), Some(&contact));
        assert_eq!(bucket.get(second_contact.id()), Some(&second_contact));
    }
}
