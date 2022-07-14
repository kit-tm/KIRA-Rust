use std::error::Error;
use std::fmt::{Display, Formatter};
use std::ops::Index;

use crate::domain::{Contact, NodeId};

pub const DEFAULT_BUCKET_SIZE: usize = 20;

#[derive(Debug)]
pub enum BucketInsertionError {
    DuplicateId(NodeId),
    Full,
}

impl Display for BucketInsertionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full => write!(f, "Tried inserting into full bucket"),
            Self::DuplicateId(id) => write!(f, "Contact with id {} already in bucket", id),
        }
    }
}

impl Error for BucketInsertionError {}

#[derive(Debug)]
pub enum ReplacementError {
    NotFound(NodeId),
    DuplicateId(NodeId),
}

impl Display for ReplacementError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "No contact to replace with id {}", id),
            Self::DuplicateId(id) => write!(f, "Contact with id {} already in bucket", id),
        }
    }
}

impl Error for ReplacementError {}

/// A [Bucket] with fixed size used in the [crate::domain::RoutingTable].
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Bucket<const SIZE: usize = DEFAULT_BUCKET_SIZE> {
    contacts: [Option<Contact>; SIZE],
}

impl<const SIZE: usize> Default for Bucket<SIZE> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const SIZE: usize> Bucket<SIZE> {
    /// Create a new [Bucket] with size of [DEFAULT_BUCKET_SIZE].
    pub fn new() -> Self {
        Self {
            // Workaround for Contact not implementing Copy
            contacts: [(); SIZE].map(|_| Option::default()),
        }
    }

    /// Returns if the [Bucket] contains any [Contact].
    pub fn is_empty(&self) -> bool {
        !self.contacts.iter().any(|entry| entry.is_some())
    }

    /// Returns if the [Bucket] has reached it's maximum size.
    pub fn is_full(&self) -> bool {
        !self.contacts.iter().any(|entry| entry.is_none())
    }

    /// Gets a [Contact] in the [Bucket] by [NodeId].
    pub fn get(&self, id: &NodeId) -> Option<&Contact> {
        self.contacts
            .iter()
            .flatten()
            .find(|contact| contact.id() == id)
    }

    /// Returns the number of [Contact]s in the [Bucket].
    pub fn len(&self) -> usize {
        self.contacts.iter().flatten().count()
    }

    /// Returns the maximum size of the [Bucket].
    pub const fn max_size(&self) -> usize {
        SIZE
    }

    /// Returns if a [Contact] with a given [NodeId] is in the [Bucket].
    pub fn contains(&self, id: &NodeId) -> bool {
        self.contacts
            .iter()
            .flatten()
            .any(|contact| contact.id() == id)
    }

    /// Returns a mutable reference to the place of the [Contact] with the given [NodeId]
    /// if present in the [Bucket].
    pub fn get_mut(&mut self, id: &NodeId) -> Option<&mut Contact> {
        self.contacts
            .iter_mut()
            .flatten()
            .find(|contact| contact.id() == id)
    }

    fn empty_entry_mut(&mut self) -> Option<&mut Option<Contact>> {
        self.contacts.iter_mut().find(|entry| entry.is_none())
    }

    /// Tries to insert a [Contact] into the [Bucket].
    pub fn insert(&mut self, contact: Contact) -> Result<(), BucketInsertionError> {
        if self.contains(contact.id()) {
            return Err(BucketInsertionError::DuplicateId(contact.into_id()));
        }

        match self.empty_entry_mut() {
            Some(entry) => {
                *entry = Some(contact);
                Ok(())
            }
            None => Err(BucketInsertionError::Full),
        }
    }

    /// Replaces a [Contact] with the given [NodeId] with another [Contact].
    /// Returns if the Replacement was successful.
    pub fn replace(
        &mut self,
        replace_id: &NodeId,
        with: Contact,
    ) -> Result<Contact, ReplacementError> {
        if !self.contains(replace_id) {
            return Err(ReplacementError::NotFound(replace_id.clone()));
        }

        if self.contains(with.id()) {
            return Err(ReplacementError::DuplicateId(replace_id.clone()));
        }

        let contact = self.get_mut(replace_id);
        // We checked before
        assert!(contact.is_some());
        let contact = contact.unwrap();
        let old_contact = contact.clone();
        *contact = with;

        Ok(old_contact)
    }

    /// Removes a [Contact] from the [Bucket] returning it if present.
    pub fn remove(&mut self, id: &NodeId) -> Option<Contact> {
        self.contacts.iter_mut().find_map(|contact| {
            if contact.is_some() && contact.as_ref().unwrap().id() == id {
                contact.take()
            } else {
                None
            }
        })
    }

    /// Splits a [Bucket] by moving contacts into another [Bucket].
    ///
    /// The **predicate** must only return *true* if the
    /// given [Contact] should be moved to the other [Bucket].
    ///
    /// Returns an [InsertionError] if inserting into the new [Bucket]
    /// fails for any reason.
    pub fn split<F, const OTHER_SIZE: usize>(
        &mut self,
        other: &mut Bucket<OTHER_SIZE>,
        mut predicate: F,
    ) -> Result<(), BucketInsertionError>
    where
        F: FnMut(&Contact) -> bool,
    {
        for contact in &mut self.contacts {
            if contact.is_some() && predicate(contact.as_ref().unwrap()) {
                other.insert(contact.take().unwrap())?;
            }
        }

        Ok(())
    }

    /// Returns an iterator over the contacts in this bucket.
    pub fn iter(&self) -> impl Iterator<Item = &Contact> {
        self.contacts.iter().flatten()
    }

    /// Returns an mutable iterator over the contacts in this bucket.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Contact> {
        self.contacts.iter_mut().flatten()
    }

    pub(crate) fn get_by_index(&self, index: usize) -> Option<&Contact> {
        self.contacts.index(index).as_ref()
    }
}

impl<const SIZE: usize> From<[Contact; SIZE]> for Bucket<SIZE> {
    fn from(array: [Contact; SIZE]) -> Self {
        Self {
            contacts: array.map(Some),
        }
    }
}

impl<const SIZE: usize> Display for Bucket<SIZE> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let contacts = self
            .iter()
            .map(|contact| contact.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        write!(f, "Bucket [{}]", contacts)
    }
}

impl<'a, const SIZE: usize> IntoIterator for &'a mut Bucket<SIZE> {
    type Item = &'a mut Contact;
    type IntoIter = Iter<&'a mut Contact>;

    fn into_iter(self) -> Self::IntoIter {
        Iter(self.contacts.iter_mut().flatten().rev().collect())
    }
}

impl<'a, const SIZE: usize> IntoIterator for &'a Bucket<SIZE> {
    type Item = &'a Contact;
    type IntoIter = Iter<&'a Contact>;

    fn into_iter(self) -> Self::IntoIter {
        Iter(self.contacts.iter().flatten().rev().collect())
    }
}

impl<const SIZE: usize> IntoIterator for Bucket<SIZE> {
    type Item = Contact;
    type IntoIter = Iter<Contact>;

    fn into_iter(self) -> Self::IntoIter {
        let mut inner = Vec::from_iter(self.contacts.into_iter().flatten());
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
    use crate::domain::{Age, Bucket, Contact, NodeId, Path, ReplacementError, StateSeqNr};

    #[test]
    fn insert_test() {
        let mut bucket = Bucket::<2>::new();

        assert!(bucket.is_empty());
        assert!(!bucket.is_full());

        let contact = Contact::new(Path::from(NodeId::one()), Age::from(0), StateSeqNr::from(0));

        assert!(bucket.insert(contact.clone()).is_ok());

        assert!(!bucket.is_empty());
        assert!(!bucket.is_full());
        assert!(bucket.contains(contact.id()));
        assert_eq!(bucket.get(contact.id()), Some(&contact));

        assert!(bucket.insert(contact.clone()).is_err());

        let second_contact = Contact::new(
            Path::from(NodeId::with_lsb(2)),
            Age::from(0),
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

    #[test]
    fn test_replacement() {
        let mut bucket = Bucket::<2>::new();

        assert!(bucket.is_empty());
        assert!(!bucket.is_full());

        let contact = Contact::new(
            Path::from(NodeId::with_lsb(1)),
            Age::from(0),
            StateSeqNr::from(0),
        );

        assert!(matches!(
            bucket.replace(&NodeId::with_lsb(1), contact.clone()),
            Err(ReplacementError::NotFound(_))
        ));

        assert!(bucket.insert(contact.clone()).is_ok());

        let contact_two = Contact::new(
            Path::from(NodeId::with_lsb(2)),
            Age::from(0),
            StateSeqNr::from(0),
        );

        assert!(bucket.replace(contact.id(), contact_two.clone()).is_ok());
        assert_eq!(bucket.len(), 1);
        assert!(bucket.contains(contact_two.id()));
    }

    #[test]
    fn split() {
        let mut bucket = Bucket::<2>::new();

        let contact = Contact::new(
            Path::from(NodeId::with_lsb(1)),
            Age::from(0),
            StateSeqNr::from(0),
        );
        assert!(bucket.insert(contact.clone()).is_ok());

        let second_contact = Contact::new(
            Path::from(NodeId::with_lsb(2)),
            Age::from(0),
            StateSeqNr::from(0),
        );
        assert!(bucket.insert(second_contact.clone()).is_ok());

        let mut other = Bucket::<2>::new();

        assert!(bucket
            .split(&mut other, |contact| contact.id() == second_contact.id())
            .is_ok());

        assert_eq!(bucket.len(), 1);
        assert!(bucket.contains(contact.id()));
        assert!(!bucket.contains(second_contact.id()));

        assert_eq!(other.len(), 1);
        assert!(!other.contains(contact.id()));
        assert!(other.contains(second_contact.id()));
    }
}
