use std::collections::HashMap;

use rand::Rng;

use crate::domain::{
    AddError, Bucket, BucketSplitError, Contact, FlatRoutingTable, NodeId, ReplacementError,
    RoutingTable,
};

/// A routing table which uses an additional data structure to store all
/// physical neighbors.
///
/// In contrast to [FlatRoutingTable] this implementation doesn't replace
/// existing contacts with physical neighbors.
/// Physical Neighbors will be added as long as they're not already present in the table.
///
/// # Invariant
///
/// No physical neighbors are in the inner routing table.
pub struct UnlimitedPNRoutingTable<const BUCKET_SIZE: usize, const ACC: usize> {
    pn_contacts: HashMap<NodeId, Contact>,
    inner: FlatRoutingTable<BUCKET_SIZE, ACC>,
}

impl<const BUCKET_SIZE: usize, const ACC: usize> From<FlatRoutingTable<BUCKET_SIZE, ACC>>
    for UnlimitedPNRoutingTable<BUCKET_SIZE, ACC>
{
    fn from(routing_table: FlatRoutingTable<BUCKET_SIZE, ACC>) -> Self {
        Self {
            pn_contacts: Default::default(),
            inner: routing_table,
        }
    }
}

pub struct Iter<'a> {
    iter: Vec<&'a Contact>,
}

impl<'a> Iter<'a> {
    fn new<const BUCKET_SIZE: usize, const ACC: usize>(
        table: &'a UnlimitedPNRoutingTable<BUCKET_SIZE, ACC>,
    ) -> Self {
        let mut iter = table
            .pn_contacts
            .values()
            .into_iter()
            .chain(table.inner.into_iter())
            .collect::<Vec<_>>();
        iter.reverse();
        Self { iter }
    }
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a Contact;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.pop()
    }
}

impl<'a, const BUCKET_SIZE: usize, const ACC: usize> IntoIterator
    for &'a UnlimitedPNRoutingTable<BUCKET_SIZE, ACC>
{
    type Item = &'a Contact;
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        Iter::new(self)
    }
}

impl<const BUCKET_SIZE: usize, const ACC: usize> RoutingTable<BUCKET_SIZE>
    for UnlimitedPNRoutingTable<BUCKET_SIZE, ACC>
{
    fn root(&self) -> &NodeId {
        self.inner.root()
    }

    fn len(&self) -> usize {
        self.pn_contacts.len() + self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.pn_contacts.is_empty() && self.inner.is_empty()
    }

    fn add(&mut self, contact: Contact) -> Result<(), AddError> {
        // Add to physical neighbors if possible
        if contact.is_pn() {
            if self.pn_contacts.contains_key(contact.id()) {
                return Err(AddError::AlreadyExists(contact.into_id()));
            }
            self.pn_contacts.insert(contact.id().clone(), contact);
            return Ok(());
        }

        // otherwise regular add
        self.inner.add(contact)
    }

    fn remove(&mut self, id: &NodeId) -> Option<Contact> {
        self.pn_contacts
            .remove(id)
            .or_else(|| self.inner.remove(id))
    }

    fn replace(&mut self, id: &NodeId, with: Contact) -> Result<(), ReplacementError> {
        if self.pn_contacts.insert(id.clone(), with.clone()).is_some() {
            return Ok(());
        }

        self.inner.replace(id, with)
    }

    fn contact(&self, id: &NodeId) -> Option<&Contact> {
        self.pn_contacts.get(id).or_else(|| self.inner.contact(id))
    }

    fn random_id(&self) -> Option<&NodeId> {
        if self.is_empty() {
            return None;
        }
        let random = rand::thread_rng().gen_range(0..self.len());
        self.into_iter().nth(random).map(|contact| contact.id())
    }

    fn contact_mut(&mut self, id: &NodeId) -> Option<&mut Contact> {
        self.pn_contacts
            .get_mut(id)
            .or_else(|| self.inner.contact_mut(id))
    }

    fn contains(&self, id: &NodeId) -> bool {
        self.pn_contacts.contains_key(id) || self.inner.contains(id)
    }

    fn split_bucket(&mut self, id: &NodeId) -> Result<(), BucketSplitError> {
        self.inner.split_bucket(id)
    }

    fn bucket(&self, of: &NodeId) -> &Bucket<BUCKET_SIZE> {
        self.inner.bucket(of)
    }

    fn bucket_mut(&mut self, of: &NodeId) -> &mut Bucket<BUCKET_SIZE> {
        self.inner.bucket_mut(of)
    }
}
