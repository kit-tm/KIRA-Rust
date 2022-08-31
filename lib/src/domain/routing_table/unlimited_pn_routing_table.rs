use std::cmp::Ordering;
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
#[derive(Debug)]
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

impl<'a, const BUCKET_SIZE: usize, const ACC: usize> RoutingTable<'a, BUCKET_SIZE>
    for UnlimitedPNRoutingTable<BUCKET_SIZE, ACC>
{
    type ContactWriteGuard = &'a mut Contact;
    type BucketWriteGuard = &'a mut Bucket<BUCKET_SIZE>;

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

    fn replace(&mut self, id: &NodeId, with: Contact) -> Result<Contact, ReplacementError> {
        if let Some(contact) = self.pn_contacts.insert(id.clone(), with.clone()) {
            return Ok(contact);
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

    fn contact_mut(&'a mut self, id: &NodeId) -> Option<Self::ContactWriteGuard> {
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

    fn bucket_mut(&'a mut self, of: &NodeId) -> Self::BucketWriteGuard {
        self.inner.bucket_mut(of)
    }

    fn get_closest(&self, to: &NodeId, shared_prefix_grouping: usize) -> Option<&Contact> {
        let bucket_closest = self.inner.get_closest(to, shared_prefix_grouping);
        let closest_neighbor =
            super::get_closest_in(self.pn_contacts.values(), to, shared_prefix_grouping)
                .expect("grouping should have been checked before");
        match (bucket_closest, closest_neighbor) {
            (Some(closest_contact), Some(closest_neighbor)) => {
                let contact_distance = closest_contact
                    .id()
                    .shared_prefix_len(to, shared_prefix_grouping)
                    .expect("Grouping should have been checked before");
                let neighbor_distance = closest_contact
                    .id()
                    .shared_prefix_len(to, shared_prefix_grouping)
                    .expect("Grouping should have been checked before");

                match contact_distance.bit_len().cmp(&neighbor_distance.bit_len()) {
                    Ordering::Greater => return Some(closest_contact),
                    Ordering::Less => return Some(closest_neighbor),
                    _ => {}
                };

                if closest_contact.id() < closest_neighbor.id() {
                    return Some(closest_contact);
                }

                Some(closest_neighbor)
            }
            (None, closest_neighbor) => closest_neighbor,
            (closest_contact, None) => closest_contact,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::unlimited_pn_routing_table::UnlimitedPNRoutingTable;
    use crate::domain::{
        Age, Contact, ContactState, FlatRoutingTable, NodeId, Path, RoutingTable, StateSeqNr,
    };

    #[test]
    fn get_closest_contact() {
        let mut invalid_contact = Contact::new(
            Path::from([NodeId::with_msb(4)]),
            Age::from(1),
            StateSeqNr::from(0),
        );
        *invalid_contact.state_mut() = ContactState::Invalid;

        let contacts = vec![
            invalid_contact,
            Contact::new(
                Path::from([NodeId::with_msb(5)]),
                Age::from(1),
                StateSeqNr::from(0),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(4), NodeId::with_msb(1)]),
                Age::from(1),
                StateSeqNr::from(0),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(5), NodeId::with_msb(2)]),
                Age::from(1),
                StateSeqNr::from(0),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(4), NodeId::with_msb(3)]),
                Age::from(1),
                StateSeqNr::from(0),
            ),
        ];

        let mut routing_table =
            UnlimitedPNRoutingTable::from(FlatRoutingTable::default(NodeId::zero()));
        routing_table
            .extend(false, contacts.clone())
            .expect("failed to insert all contact");

        let closest = routing_table.get_closest(&NodeId::zero(), 1);
        assert!(closest.is_some(), "Returned None");
        let closest = closest.unwrap();
        assert_eq!(closest, &contacts[2]);
    }

    #[test]
    fn get_closest_neighbor() {
        let mut invalid_contact = Contact::new(
            Path::from([NodeId::with_msb(2)]),
            Age::from(1),
            StateSeqNr::from(0),
        );
        *invalid_contact.state_mut() = ContactState::Invalid;

        let contacts = vec![
            invalid_contact,
            Contact::new(
                Path::from([NodeId::with_msb(3)]),
                Age::from(1),
                StateSeqNr::from(0),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(2), NodeId::with_msb(4)]),
                Age::from(1),
                StateSeqNr::from(0),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(3), NodeId::with_msb(5)]),
                Age::from(1),
                StateSeqNr::from(0),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(2), NodeId::with_msb(6)]),
                Age::from(1),
                StateSeqNr::from(0),
            ),
        ];

        let mut routing_table =
            UnlimitedPNRoutingTable::from(FlatRoutingTable::default(NodeId::zero()));
        routing_table
            .extend(false, contacts.clone())
            .expect("failed to insert all contact");

        let closest = routing_table.get_closest(&NodeId::zero(), 1);
        assert!(closest.is_some(), "Returned None");
        let closest = closest.unwrap();
        assert_eq!(closest, &contacts[1]);
    }

    #[test]
    fn get_closest_in_empty() {
        let mut invalid_neighbor = Contact::new(
            Path::from([NodeId::with_msb(2)]),
            Age::from(1),
            StateSeqNr::from(0),
        );
        *invalid_neighbor.state_mut() = ContactState::Invalid;

        let mut invalid_contact = Contact::new(
            Path::from([NodeId::with_msb(2), NodeId::with_msb(4)]),
            Age::from(1),
            StateSeqNr::from(0),
        );
        *invalid_contact.state_mut() = ContactState::Invalid;

        let contacts = vec![invalid_neighbor, invalid_contact];

        let mut routing_table =
            UnlimitedPNRoutingTable::from(FlatRoutingTable::default(NodeId::zero()));
        routing_table
            .extend(false, contacts.clone())
            .expect("failed to insert all contact");

        let closest = routing_table.get_closest(&NodeId::zero(), 1);
        assert!(closest.is_none(), "Returned Some: {:?}", closest);
    }
}
