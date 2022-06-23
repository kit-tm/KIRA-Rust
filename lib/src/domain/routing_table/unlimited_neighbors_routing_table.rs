use std::collections::HashMap;

use rand::Rng;

use crate::domain::{
    AddError, Bucket, BucketSplitError, Contact, FlatRoutingTable, NodeId, ReplacementError,
    RoutingTable,
};

/// A routing table which uses an additional data structure to store
/// physical neighbors.
///
/// In contrast to [FlatRoutingTable] this implementation doesn't replace
/// existing contacts with neighbors.
/// Neighbors will be added as long as they're not already present in the table.
///
/// # Invariant
///
/// No neighbors are in the inner routing table.
pub struct UnlimitedNeighborsRoutingTable<
    const ID_SIZE: usize,
    const BUCKET_SIZE: usize,
    const ACC: usize,
> {
    neighbor_contacts: HashMap<NodeId<ID_SIZE>, Contact<ID_SIZE>>,
    inner: FlatRoutingTable<ID_SIZE, BUCKET_SIZE, ACC>,
}

impl<const ID_SIZE: usize, const BUCKET_SIZE: usize, const ACC: usize>
    From<FlatRoutingTable<ID_SIZE, BUCKET_SIZE, ACC>>
    for UnlimitedNeighborsRoutingTable<ID_SIZE, BUCKET_SIZE, ACC>
{
    fn from(routing_table: FlatRoutingTable<ID_SIZE, BUCKET_SIZE, ACC>) -> Self {
        Self {
            neighbor_contacts: Default::default(),
            inner: routing_table,
        }
    }
}

pub struct Iter<'a, const ID_SIZE: usize> {
    iter: Vec<&'a Contact<ID_SIZE>>,
}

impl<'a, const ID_SIZE: usize> Iter<'a, ID_SIZE> {
    fn new<const BUCKET_SIZE: usize, const ACC: usize>(
        table: &'a UnlimitedNeighborsRoutingTable<ID_SIZE, BUCKET_SIZE, ACC>,
    ) -> Self {
        let mut iter = table
            .neighbor_contacts
            .values()
            .into_iter()
            .chain(table.inner.into_iter())
            .collect::<Vec<_>>();
        iter.reverse();
        Self { iter }
    }
}

impl<'a, const ID_SIZE: usize> Iterator for Iter<'a, ID_SIZE> {
    type Item = &'a Contact<ID_SIZE>;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.pop()
    }
}

impl<'a, const ID_SIZE: usize, const BUCKET_SIZE: usize, const ACC: usize> IntoIterator
    for &'a UnlimitedNeighborsRoutingTable<ID_SIZE, BUCKET_SIZE, ACC>
{
    type Item = &'a Contact<ID_SIZE>;
    type IntoIter = Iter<'a, ID_SIZE>;

    fn into_iter(self) -> Self::IntoIter {
        Iter::new(self)
    }
}

impl<const ID_SIZE: usize, const BUCKET_SIZE: usize, const ACC: usize>
    RoutingTable<ID_SIZE, BUCKET_SIZE>
    for UnlimitedNeighborsRoutingTable<ID_SIZE, BUCKET_SIZE, ACC>
{
    fn root(&self) -> &NodeId<ID_SIZE> {
        self.inner.root()
    }

    fn len(&self) -> usize {
        self.neighbor_contacts.len() + self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.neighbor_contacts.is_empty() && self.inner.is_empty()
    }

    fn add(&mut self, contact: Contact<ID_SIZE>) -> Result<(), AddError<ID_SIZE>> {
        // Add to neighbors if possible
        if contact.path().is_empty() {
            if self.neighbor_contacts.contains_key(contact.id()) {
                return Err(AddError::AlreadyExists(contact.into_id()));
            }
            self.neighbor_contacts.insert(contact.id().clone(), contact);
            return Ok(());
        }

        // otherwise regular add
        self.inner.add(contact)
    }

    fn remove(&mut self, id: &NodeId<ID_SIZE>) -> Option<Contact<ID_SIZE>> {
        self.neighbor_contacts
            .remove(id)
            .or_else(|| self.inner.remove(id))
    }

    fn replace(
        &mut self,
        id: &NodeId<ID_SIZE>,
        with: Contact<ID_SIZE>,
    ) -> Result<(), ReplacementError<ID_SIZE>> {
        if self
            .neighbor_contacts
            .insert(id.clone(), with.clone())
            .is_some()
        {
            return Ok(());
        }

        self.inner.replace(id, with)
    }

    fn contact(&self, id: &NodeId<ID_SIZE>) -> Option<&Contact<ID_SIZE>> {
        self.neighbor_contacts
            .get(id)
            .or_else(|| self.inner.contact(id))
    }

    fn random_id(&self) -> Option<&NodeId<ID_SIZE>> {
        let random = rand::thread_rng().gen_range(0..self.len());
        self.into_iter().nth(random).map(|contact| contact.id())
    }

    fn contact_mut(&mut self, id: &NodeId<ID_SIZE>) -> Option<&mut Contact<ID_SIZE>> {
        self.neighbor_contacts
            .get_mut(id)
            .or_else(|| self.inner.contact_mut(id))
    }

    fn contains(&self, id: &NodeId<ID_SIZE>) -> bool {
        self.neighbor_contacts.contains_key(id) || self.inner.contains(id)
    }

    fn split_bucket(&mut self, id: &NodeId<ID_SIZE>) -> Result<(), BucketSplitError> {
        self.inner.split_bucket(id)
    }

    fn bucket(&self, of: &NodeId<ID_SIZE>) -> &Bucket<ID_SIZE, BUCKET_SIZE> {
        self.inner.bucket(of)
    }

    fn bucket_mut(&mut self, of: &NodeId<ID_SIZE>) -> &mut Bucket<ID_SIZE, BUCKET_SIZE> {
        self.inner.bucket_mut(of)
    }
}
