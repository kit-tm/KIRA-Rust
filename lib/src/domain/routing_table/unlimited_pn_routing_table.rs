use std::collections::hash_map::{Values, ValuesMut};
use std::collections::HashMap;

use rand::Rng;

use crate::domain::{AddError, Bucket, BucketSplitError, Contact, ContactState, DiscoveryRangeProvider, FlatRoutingTable, GroupingError, NodeId, ReplacementError, RoutingTable, SharedPrefix};
use crate::domain::api::DiscoveryRange;

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
#[derive(Debug, Clone)]
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

impl<const BUCKET_SIZE: usize, const ACC: usize> From<UnlimitedPNRoutingTable<BUCKET_SIZE, ACC>> for crate::domain::api::RoutingTable {
    fn from(value: UnlimitedPNRoutingTable<BUCKET_SIZE, ACC>) -> Self {
        value.inner.into()
    }
}

pub struct Iter<'a, const BUCKET_SIZE: usize, const ACC: usize> {
    pn_iter: Values<'a, NodeId, Contact>,
    inner_iter: <FlatRoutingTable<BUCKET_SIZE, ACC> as RoutingTable<'a, BUCKET_SIZE>>::Iter,
}

impl<'a, const BUCKET_SIZE: usize, const ACC: usize>
    From<&'a UnlimitedPNRoutingTable<BUCKET_SIZE, ACC>> for Iter<'a, BUCKET_SIZE, ACC>
{
    fn from(table: &'a UnlimitedPNRoutingTable<BUCKET_SIZE, ACC>) -> Self {
        Self {
            pn_iter: table.pn_contacts.values(),
            inner_iter: table.inner.iter(),
        }
    }
}

impl<'a, const BUCKET_SIZE: usize, const ACC: usize> Iterator for Iter<'a, BUCKET_SIZE, ACC> {
    type Item = &'a Contact;

    fn next(&mut self) -> Option<Self::Item> {
        self.pn_iter.next().or_else(|| self.inner_iter.next())
    }
}

pub struct IterMut<'a, const BUCKET_SIZE: usize, const ACC: usize> {
    pn_iter: ValuesMut<'a, NodeId, Contact>,
    inner_iter: <FlatRoutingTable<BUCKET_SIZE, ACC> as RoutingTable<'a, BUCKET_SIZE>>::IterMut,
}

impl<'a, const BUCKET_SIZE: usize, const ACC: usize>
    From<&'a mut UnlimitedPNRoutingTable<BUCKET_SIZE, ACC>> for IterMut<'a, BUCKET_SIZE, ACC>
{
    fn from(table: &'a mut UnlimitedPNRoutingTable<BUCKET_SIZE, ACC>) -> Self {
        Self {
            pn_iter: table.pn_contacts.values_mut(),
            inner_iter: table.inner.iter_mut(),
        }
    }
}

impl<'a, const BUCKET_SIZE: usize, const ACC: usize> Iterator for IterMut<'a, BUCKET_SIZE, ACC> {
    type Item = &'a mut Contact;

    fn next(&mut self) -> Option<Self::Item> {
        self.pn_iter.next().or_else(|| self.inner_iter.next())
    }
}

impl<const BUCKET_SIZE: usize, const ACC: usize> DiscoveryRangeProvider for UnlimitedPNRoutingTable<BUCKET_SIZE, ACC> {
    fn get_discovery_range(&self) -> DiscoveryRange {
        self.inner.get_discovery_range()
    }
}

impl<'a, const BUCKET_SIZE: usize, const ACC: usize> RoutingTable<'a, BUCKET_SIZE>
    for UnlimitedPNRoutingTable<BUCKET_SIZE, ACC>
{
    type ContactWriteGuard = &'a mut Contact;
    type BucketWriteGuard = &'a mut Bucket<BUCKET_SIZE>;
    type Iter = Iter<'a, BUCKET_SIZE, ACC>;
    type IterMut = IterMut<'a, BUCKET_SIZE, ACC>;

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
        self.iter().nth(random).map(|contact| contact.id())
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

    fn closest(
        &self,
        to: &NodeId,
        n: usize,
        shared_prefix_grouping: usize,
    ) -> Result<Vec<(SharedPrefix, Contact)>, GroupingError> {
        let mut closest = self.inner.closest(to, n, shared_prefix_grouping)?;

        // For every neighbor find the place to insert it if possible
        for (id, contact) in &self.pn_contacts {
            if contact.state() != &ContactState::Valid {
                continue;
            }

            let prefix = to
                .shared_prefix_len(id, shared_prefix_grouping)
                .expect("grouping should be checked before");

            if closest.is_empty() {
                closest.push((prefix, contact.clone()));
                continue;
            }

            // Find position of contact
            let position = closest
                .iter()
                .position(|(closest_prefix, closest_contact)| {
                    if closest_prefix > &prefix {
                        return true;
                    }
                    if closest_prefix == &prefix && closest_contact.id() > id {
                        return true;
                    }
                    false
                });
            if let Some(index) = position {
                if closest.len() == n {
                    closest.remove(closest.len() - 1);
                }
                closest.insert(index, (prefix, contact.clone()));
            }
        }

        Ok(closest)
    }

    fn iter(&'a self) -> Self::Iter {
        Iter::from(self)
    }

    fn iter_mut(&'a mut self) -> Self::IterMut {
        IterMut::from(self)
    }


    fn is_in_last_two_buckets(&self, node_id: &NodeId) -> bool {
        self.inner.is_in_last_two_buckets(node_id)
    }

    fn range_last_two_buckets(&self) -> (NodeId, NodeId) {
        self.inner.range_last_two_buckets()
    }

}

#[cfg(test)]
mod tests {
    use crate::domain::unlimited_pn_routing_table::UnlimitedPNRoutingTable;
    use crate::domain::{
        Contact, ContactState, FlatRoutingTable, NodeId, Path, RoutingTable, StateSeqNr,
    };

    #[test]
    fn get_closest_contact() {
        let mut invalid_contact =
            Contact::new(Path::from([NodeId::with_msb(4)]), StateSeqNr::from(0));
        *invalid_contact.state_mut() = ContactState::Invalid;

        let contacts = vec![
            invalid_contact,
            Contact::new(Path::from([NodeId::with_msb(5)]), StateSeqNr::from(0)),
            Contact::new(
                Path::from([NodeId::with_msb(4), NodeId::with_msb(1)]),
                StateSeqNr::from(0),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(5), NodeId::with_msb(2)]),
                StateSeqNr::from(0),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(4), NodeId::with_msb(3)]),
                StateSeqNr::from(0),
            ),
        ];

        let mut routing_table =
            UnlimitedPNRoutingTable::from(FlatRoutingTable::default(NodeId::zero()));
        routing_table
            .extend(false, contacts.clone())
            .expect("failed to insert all contact");

        let closest = routing_table.closest(&NodeId::zero(), 20, 1);
        assert!(closest.is_ok(), "Returned None");
        let closest = closest.unwrap();
        let first = closest.first();
        assert!(first.is_some(), "Returned no closest contacts");
        let (_, contact) = first.unwrap();
        assert_eq!(
            contact, &contacts[2],
            "Returned strange order of closest contacts: {:#?}",
            closest
        );
    }

    #[test]
    fn get_closest_neighbor() {
        let mut invalid_contact =
            Contact::new(Path::from([NodeId::with_msb(2)]), StateSeqNr::from(0));
        *invalid_contact.state_mut() = ContactState::Invalid;

        let contacts = vec![
            invalid_contact,
            Contact::new(Path::from([NodeId::with_msb(3)]), StateSeqNr::from(0)),
            Contact::new(
                Path::from([NodeId::with_msb(2), NodeId::with_msb(4)]),
                StateSeqNr::from(0),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(3), NodeId::with_msb(5)]),
                StateSeqNr::from(0),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(2), NodeId::with_msb(6)]),
                StateSeqNr::from(0),
            ),
        ];

        let mut routing_table =
            UnlimitedPNRoutingTable::from(FlatRoutingTable::default(NodeId::zero()));
        routing_table
            .extend(false, contacts.clone())
            .expect("failed to insert all contact");

        let closest = routing_table.closest(&NodeId::zero(), 1, 1);
        assert!(closest.is_ok(), "Returned None");
        let closest = closest.unwrap();
        let first = closest.first();
        assert!(first.is_some(), "Returned no closest contacts");
        let (_, contact) = first.unwrap();
        assert_eq!(
            contact, &contacts[1],
            "Returned strange order of closest contacts: {:#?}",
            closest
        );
    }

    #[test]
    fn get_closest_of_only_neighbors() {
        let contacts = vec![
            Contact::new(Path::from([NodeId::with_msb(5)]), StateSeqNr::from(0)),
            Contact::new(Path::from([NodeId::with_msb(6)]), StateSeqNr::from(0)),
            Contact::new(Path::from([NodeId::with_msb(3)]), StateSeqNr::from(0)),
            Contact::new(Path::from([NodeId::with_msb(4)]), StateSeqNr::from(0)),
        ];

        let mut routing_table =
            UnlimitedPNRoutingTable::from(FlatRoutingTable::default(NodeId::zero()));
        routing_table
            .extend(false, contacts.clone())
            .expect("failed to insert all contact");

        let closest = routing_table.closest(&NodeId::zero(), 1, 1);
        assert!(closest.is_ok(), "Returned None");
        let closest = closest.unwrap();
        let first = closest.first();
        assert!(first.is_some(), "Returned no closest contacts");
        let (_, contact) = first.unwrap();
        assert_eq!(
            contact, &contacts[2],
            "Returned strange order of closest contacts: {:#?}",
            closest
        );
    }

    #[test]
    fn get_closest_in_empty() {
        let mut invalid_neighbor =
            Contact::new(Path::from([NodeId::with_msb(2)]), StateSeqNr::from(0));
        *invalid_neighbor.state_mut() = ContactState::Invalid;

        let mut invalid_contact = Contact::new(
            Path::from([NodeId::with_msb(2), NodeId::with_msb(4)]),
            StateSeqNr::from(0),
        );
        *invalid_contact.state_mut() = ContactState::Invalid;

        let contacts = vec![invalid_neighbor, invalid_contact];

        let mut routing_table =
            UnlimitedPNRoutingTable::from(FlatRoutingTable::default(NodeId::zero()));
        routing_table
            .extend(false, contacts.clone())
            .expect("failed to insert all contact");

        let closest = routing_table.closest(&NodeId::zero(), 20, 1);
        assert!(closest.is_ok(), "Returned error: {:?}", closest);
        let closest = closest.unwrap();
        assert!(
            closest.is_empty(),
            "Returned closest contacts: {:#?}",
            closest
        );
    }
}
