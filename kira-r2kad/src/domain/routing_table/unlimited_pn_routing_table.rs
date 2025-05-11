use std::collections::HashMap;

use rand::Rng;

use crate::domain::{
    AddError, Bucket, BucketSplitError, Contact, ContactState, FlatRoutingTable, GroupingError,
    NodeId, ReplacementError, RoutingTable, SharedPrefix,
};

/// A routing table which uses an additional data structure to store all
/// underlay neighbors.
///
/// In contrast to [FlatRoutingTable] this implementation doesn't replace
/// existing contacts with underlay neighbors.
/// Underlay Neighbors will be added as long as they're not already present in the table.
///
/// # Invariant
///
/// No underlay neighbors are in the inner routing table.
///
/// # Important
///
/// Calling [`bucket_iter`](UnlimitedPNRoutingTable::bucket_iter) will **not** yield
/// any underlay neighbors.
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

impl<'a, const BUCKET_SIZE: usize, const ACC: usize> RoutingTable<'a, BUCKET_SIZE>
    for UnlimitedPNRoutingTable<BUCKET_SIZE, ACC>
{
    type ContactWriteGuard = &'a mut Contact;
    type BucketWriteGuard = &'a mut Bucket<BUCKET_SIZE>;
    type BucketIter = std::slice::Iter<'a, Bucket<BUCKET_SIZE>>;

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
        // Add to underlay neighbors if possible
        if contact.is_pn() {
            if self.pn_contacts.contains_key(contact.id()) {
                return Err(AddError::AlreadyExists(contact.into_id()));
            }
            self.pn_contacts.insert(*contact.id(), contact);
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
        if let Some(contact) = self.pn_contacts.insert(*id, with.clone()) {
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

    fn split_bucket(&mut self, id: &NodeId) -> Result<usize, BucketSplitError> {
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

    fn iter(&self) -> impl Iterator<Item = &Contact> {
        self.pn_contacts.values().chain(self.inner.iter())
    }

    fn iter_mut(&'a mut self) -> impl Iterator<Item = Self::ContactWriteGuard> {
        self.pn_contacts.values_mut().chain(self.inner.iter_mut())
    }

    fn bucket_iter(&'a self) -> Self::BucketIter {
        self.inner.bucket_iter()
    }

    fn bucket_by_index(&self, index: usize) -> &Bucket<BUCKET_SIZE> {
        self.inner.bucket_by_index(index)
    }

    fn bucket_by_index_mut(&'a mut self, index: usize) -> Self::BucketWriteGuard {
        self.inner.bucket_by_index_mut(index)
    }

    fn get_bucket_index(&self, of: &NodeId) -> usize {
        self.inner.get_bucket_index(of)
    }

    fn get_bucket_prefix_length(&self, bucket_index: usize) -> usize {
        self.inner.get_bucket_prefix_length(bucket_index)
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::unlimited_pn_routing_table::UnlimitedPNRoutingTable;
    use crate::domain::{
        Contact, ContactState, FlatRoutingTable, NodeId, Path, RoutingTable, StateSeqNr,
    };

    #[test]
    fn yield_physical_neighbors() {
        let invalid_contact = Contact::new(Path::from([NodeId::with_msb(4)]), StateSeqNr::from(0));

        let mut routing_table =
            UnlimitedPNRoutingTable::from(FlatRoutingTable::default(NodeId::zero()));

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

        routing_table
            .extend(false, contacts.clone())
            .expect("failed to insert all contacts");

        assert_eq!(
            routing_table.iter().collect::<Vec<_>>().len(),
            contacts.len(),
            "yield all contacts"
        );

        assert_eq!(
            routing_table.iter_mut().collect::<Vec<_>>().len(),
            contacts.len(),
            "yield all contacts"
        );
    }
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
