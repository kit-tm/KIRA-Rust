use std::{collections::HashMap, num::NonZeroU8};

use rand::Rng;
use tracing::Level;

use crate::domain::{
    AddError, Bucket, BucketSplitError, Contact, ContactState, FlatRoutingTable, GroupingError,
    NodeId, ReplacementError, RoutingTable,
    hasher::Hasher,
    routing_table::{PrefixContact, sorter_xor},
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
/// Calling [`bucket_iter`](UnlimitedULNRoutingTable::bucket_iter) will **not** yield
/// any underlay neighbors.
#[derive(Debug)]
pub struct UnlimitedULNRoutingTable<const BUCKET_SIZE: usize, const ACC: u8> {
    un_contacts: HashMap<NodeId, Contact>,
    inner: FlatRoutingTable<BUCKET_SIZE, ACC>,
}

impl<const BUCKET_SIZE: usize, const ACC: u8> From<FlatRoutingTable<BUCKET_SIZE, ACC>>
    for UnlimitedULNRoutingTable<BUCKET_SIZE, ACC>
{
    fn from(routing_table: FlatRoutingTable<BUCKET_SIZE, ACC>) -> Self {
        assert!(routing_table.is_empty(), "FlatRoutingTable should be empty");

        Self {
            un_contacts: Default::default(),
            inner: routing_table,
        }
    }
}

impl<'a, const BUCKET_SIZE: usize, const ACC: u8> RoutingTable<'a, BUCKET_SIZE>
    for UnlimitedULNRoutingTable<BUCKET_SIZE, ACC>
{
    type ContactWriteGuard = &'a mut Contact;
    type BucketIter = std::slice::Iter<'a, Bucket<BUCKET_SIZE>>;

    fn root(&self) -> &NodeId {
        self.inner.root()
    }

    fn len(&self) -> usize {
        self.un_contacts.len() + self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.un_contacts.is_empty() && self.inner.is_empty()
    }

    fn add(&mut self, contact: Contact) -> Result<(), AddError> {
        if let Some(uln_entry) = self.un_contacts.get(contact.id()) {
            // contact to add exists as ULN already
            if contact.is_uln() {
                return Err(AddError::AlreadyExists(contact.into_id()));
            }
            // the contact is a ULN, but has a longer path now, should be ignored by update
            if *uln_entry.state() == ContactState::Valid {
                return Err(AddError::AlreadyExists(contact.into_id()));
            } else {
                // if existing contact was former ULN, but it is not anymore, we may remove it from there
                self.un_contacts.remove(contact.id());
            }
        } else {
            // contact does not exist as ULN
            if contact.is_uln() {
                // add to underlay neighbors
                _ = self.inner.remove(contact.id());
                self.un_contacts.insert(*contact.id(), contact);
                return Ok(());
            }
        }
        // try regular add, returns own result
        self.inner.add(contact)
    }

    fn remove(&mut self, id: &NodeId) -> Option<Contact> {
        self.un_contacts
            .remove(id)
            .or_else(|| self.inner.remove(id))
    }

    fn replace(&mut self, id: &NodeId, with: Contact) -> Result<Contact, ReplacementError> {
        if let Some(contact) = self.un_contacts.insert(*id, with.clone()) {
            return Ok(contact);
        }

        self.inner.replace(id, with)
    }

    fn contact(&self, id: &NodeId) -> Option<&Contact> {
        self.un_contacts.get(id).or_else(|| self.inner.contact(id))
    }

    fn random_id(&self) -> Option<&NodeId> {
        if self.is_empty() {
            return None;
        }
        let random = rand::rng().random_range(0..self.len());
        self.iter().nth(random).map(|contact| contact.id())
    }

    fn contact_mut(&'a mut self, id: &NodeId) -> Option<Self::ContactWriteGuard> {
        self.un_contacts
            .get_mut(id)
            .or_else(|| self.inner.contact_mut(id))
    }

    fn contains(&self, id: &NodeId) -> bool {
        self.un_contacts.contains_key(id) || self.inner.contains(id)
    }

    fn split_bucket(&mut self, id: &NodeId) -> Result<usize, BucketSplitError> {
        self.inner.split_bucket(id)
    }

    fn bucket(&self, of: &NodeId) -> &Bucket<BUCKET_SIZE> {
        self.inner.bucket(of)
    }

    fn bucket_by_index(&self, index: usize) -> &Bucket<BUCKET_SIZE> {
        self.inner.bucket_by_index(index)
    }

    fn get_bucket_index(&self, of: &NodeId) -> usize {
        self.inner.get_bucket_index(of)
    }

    fn get_bucket_prefix_length(&self, bucket_index: usize) -> u8 {
        self.inner.get_bucket_prefix_length(bucket_index)
    }

    #[tracing::instrument(
        level = Level::TRACE,
        target = "routing_table::unlimited_uln_routing_table",
        skip(self),
        ret, err
    )]
    fn closest(
        &self,
        target: &NodeId,
        n: usize,
        shared_prefix_grouping: NonZeroU8,
    ) -> Result<Vec<PrefixContact>, GroupingError> {
        let mut closest = self.inner.closest(target, n, shared_prefix_grouping)?;
        closest.extend(
            self.un_contacts
                .values()
                .filter(|contact| contact.state() == &ContactState::Valid)
                .map(|contact| {
                    let prefix = target
                        .shared_prefix_len(contact.id(), shared_prefix_grouping)
                        .expect("grouping should be checked before");
                    (prefix, contact.clone())
                }),
        );
        closest.sort_unstable_by(sorter_xor);

        // Don't include additional if strict XOR-metric routing is required (lowest bucket),
        // suggesting proximity neighbor selection could be done.
        //
        // Check if the nth contact is in the lowest bucket.
        if let Some((_, nth_closest)) = closest.get(n) {
            let last_bucket_index = self.inner.num_buckets() - 1;
            let closest_bucket_index = self.inner.get_bucket_index(nth_closest.id());

            if closest_bucket_index == last_bucket_index {
                tracing::trace!(
                    target: "routing_table::unlimited_uln_routing_table",
                    reason = "last bucket contacts require sorting by strict XOR-metric",
                    "Truncate collected results",
                );
                closest.truncate(n);
            }
        }

        Ok(closest)
    }

    fn iter(&self) -> impl Iterator<Item = &Contact> {
        self.un_contacts.values().chain(self.inner.iter())
    }

    fn iter_mut(&'a mut self) -> impl Iterator<Item = Self::ContactWriteGuard> {
        self.un_contacts.values_mut().chain(self.inner.iter_mut())
    }

    fn bucket_iter(&'a self) -> Self::BucketIter {
        self.inner.bucket_iter()
    }

    fn path_hasher(&self) -> Hasher {
        self.inner.path_hasher()
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU8;

    use crate::domain::unlimited_uln_routing_table::UnlimitedULNRoutingTable;
    use crate::domain::{
        Contact, ContactState, FlatRoutingTable, NodeId, Path, RoutingTable, SafeStateSeqNr,
    };

    #[test]
    fn yield_underlay_neighbors() {
        let invalid_contact = Contact::new(
            Path::from([NodeId::with_msb(4)]),
            SafeStateSeqNr::try_from(1).unwrap(),
        );

        let mut routing_table =
            UnlimitedULNRoutingTable::from(FlatRoutingTable::default(NodeId::ZERO));

        let contacts = vec![
            invalid_contact,
            Contact::new(
                Path::from([NodeId::with_msb(5)]),
                SafeStateSeqNr::try_from(1).unwrap(),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(4), NodeId::with_msb(1)]),
                SafeStateSeqNr::try_from(1).unwrap(),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(5), NodeId::with_msb(2)]),
                SafeStateSeqNr::try_from(1).unwrap(),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(4), NodeId::with_msb(3)]),
                SafeStateSeqNr::try_from(1).unwrap(),
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
        let mut invalid_contact = Contact::new(
            Path::from([NodeId::with_msb(4)]),
            SafeStateSeqNr::try_from(1).unwrap(),
        );
        *invalid_contact.state_mut() = ContactState::Invalid;

        let contacts = vec![
            invalid_contact,
            Contact::new(
                Path::from([NodeId::with_msb(5)]),
                SafeStateSeqNr::try_from(1).unwrap(),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(4), NodeId::with_msb(1)]),
                SafeStateSeqNr::try_from(1).unwrap(),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(5), NodeId::with_msb(2)]),
                SafeStateSeqNr::try_from(1).unwrap(),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(4), NodeId::with_msb(3)]),
                SafeStateSeqNr::try_from(1).unwrap(),
            ),
        ];

        let mut routing_table =
            UnlimitedULNRoutingTable::from(FlatRoutingTable::default(NodeId::ZERO));
        routing_table
            .extend(false, contacts.clone())
            .expect("failed to insert all contacts");

        let closest = routing_table.closest(&NodeId::ZERO, 20, NonZeroU8::MIN);
        assert!(closest.is_ok(), "Returned None");
        let closest = closest.unwrap();
        let first = closest.first();
        assert!(first.is_some(), "Returned no closest contacts");
        let (_, contact) = first.unwrap();
        assert_eq!(
            contact, &contacts[2],
            "Returned strange order of closest contacts: {closest:#?}"
        );
    }

    #[test]
    fn get_closest_neighbor() {
        let mut invalid_contact = Contact::new(
            Path::from([NodeId::with_msb(2)]),
            SafeStateSeqNr::try_from(1).unwrap(),
        );
        *invalid_contact.state_mut() = ContactState::Invalid;

        let contacts = vec![
            invalid_contact,
            Contact::new(
                Path::from([NodeId::with_msb(3)]),
                SafeStateSeqNr::try_from(1).unwrap(),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(2), NodeId::with_msb(4)]),
                SafeStateSeqNr::try_from(1).unwrap(),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(3), NodeId::with_msb(5)]),
                SafeStateSeqNr::try_from(1).unwrap(),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(2), NodeId::with_msb(6)]),
                SafeStateSeqNr::try_from(1).unwrap(),
            ),
        ];

        let mut routing_table =
            UnlimitedULNRoutingTable::from(FlatRoutingTable::default(NodeId::ZERO));
        routing_table
            .extend(false, contacts.clone())
            .expect("failed to insert all contacts");

        let closest = routing_table.closest(&NodeId::ZERO, 1, NonZeroU8::MIN);
        assert!(closest.is_ok(), "Returned None");
        let closest = closest.unwrap();
        let first = closest.first();
        assert!(first.is_some(), "Returned no closest contacts");
        let (_, contact) = first.unwrap();
        assert_eq!(
            contact, &contacts[1],
            "Returned strange order of closest contacts: {closest:#?}"
        );
    }

    #[test]
    fn get_closest_of_only_neighbors() {
        let contacts = vec![
            Contact::new(
                Path::from([NodeId::with_msb(5)]),
                SafeStateSeqNr::try_from(1).unwrap(),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(6)]),
                SafeStateSeqNr::try_from(1).unwrap(),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(3)]),
                SafeStateSeqNr::try_from(1).unwrap(),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(4)]),
                SafeStateSeqNr::try_from(1).unwrap(),
            ),
        ];

        let mut routing_table =
            UnlimitedULNRoutingTable::from(FlatRoutingTable::default(NodeId::ZERO));
        routing_table
            .extend(false, contacts.clone())
            .expect("failed to insert all contacts");

        let closest = routing_table.closest(&NodeId::ZERO, 1, NonZeroU8::MIN);
        assert!(closest.is_ok(), "Returned None");
        let closest = closest.unwrap();
        let first = closest.first();
        assert!(first.is_some(), "Returned no closest contacts");
        let (_, contact) = first.unwrap();
        assert_eq!(
            contact, &contacts[2],
            "Returned strange order of closest contacts: {closest:#?}"
        );
    }

    #[test]
    fn get_closest_in_empty() {
        let mut invalid_neighbor = Contact::new(
            Path::from([NodeId::with_msb(2)]),
            SafeStateSeqNr::try_from(1).unwrap(),
        );
        *invalid_neighbor.state_mut() = ContactState::Invalid;

        let mut invalid_contact = Contact::new(
            Path::from([NodeId::with_msb(2), NodeId::with_msb(4)]),
            SafeStateSeqNr::try_from(1).unwrap(),
        );
        *invalid_contact.state_mut() = ContactState::Invalid;

        let contacts = vec![invalid_neighbor, invalid_contact];

        let mut routing_table =
            UnlimitedULNRoutingTable::from(FlatRoutingTable::default(NodeId::ZERO));
        routing_table
            .extend(false, contacts.clone())
            .expect("failed to insert all contacts");

        let closest = routing_table.closest(&NodeId::ZERO, 20, NonZeroU8::MIN);
        assert!(closest.is_ok(), "Returned error: {closest:?}");
        let closest = closest.unwrap();
        assert!(
            closest.is_empty(),
            "Returned closest contacts: {closest:#?}"
        );
    }

    #[test]
    fn get_closest_invalid_contacts() {
        crate::tests::init();
        let root = NodeId::ZERO;
        let mut table =
            UnlimitedULNRoutingTable::from(FlatRoutingTable::<1, 1>::new(root).unwrap());

        let ids = [
            NodeId::with_msb(0b1000_0000),
            NodeId::with_msb(0b1100_0000),
            NodeId::with_msb(0b0100_0000),
            NodeId::with_msb(0b0010_0000),
        ];

        for id in &ids {
            table
                .insert(Contact::new(
                    Path::from(*id),
                    SafeStateSeqNr::try_from(1).unwrap(),
                ))
                .unwrap();
        }

        *table.contact_mut(&ids[1]).unwrap().state_mut() = ContactState::Invalid;
        assert_eq!(
            table.contact(&ids[1]).unwrap().state(),
            &ContactState::Invalid,
            "1100... is invalid",
        );

        let query_id = NodeId::with_msb(0b1100_0001);
        let results: Vec<_> = table
            .closest(&query_id, 1, NonZeroU8::new(1).unwrap())
            .unwrap()
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");

        assert_eq!(results.len(), 1);

        assert_eq!(results[0].1, ids[0]);
        assert_eq!(results[0].0.bit_len(), 1);
    }

    #[test]
    fn get_closest_last_bucket() {
        crate::tests::init();
        let root = NodeId::ZERO;
        let mut table =
            UnlimitedULNRoutingTable::from(FlatRoutingTable::<2, 1>::new(root).unwrap());

        let uln = NodeId::with_msb(0b1000_0000);
        table
            .insert(Contact::new(
                Path::from(uln),
                SafeStateSeqNr::try_from(1).unwrap(),
            ))
            .unwrap();

        let ids = [NodeId::with_msb(0b1000_0010), NodeId::with_msb(0b1000_0001)];
        for id in &ids {
            table
                .insert(Contact::new(
                    Path::from([uln, *id]),
                    SafeStateSeqNr::try_from(1).unwrap(),
                ))
                .unwrap();
        }

        // All contacts have SP=1. All contacts belong into last bucket.
        //
        // Valid contacts have to be returned in strict XOR-metric.
        // Don't return additional contacts because proximity neighbor selection
        // could violate strict XOR-metric requirement.

        let query_id = NodeId::with_msb(0b1100_0001);
        let results: Vec<_> = table
            .closest(&query_id, 1, NonZeroU8::new(1).unwrap())
            .unwrap()
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");

        assert_eq!(
            results.len(),
            1,
            "no additional contacts of last bucket, strict XOR-metric"
        );

        assert_eq!(results[0].1, ids[1]);
        assert_eq!(results[0].0.bit_len(), 1);

        let query_id = NodeId::with_msb(0b1100_0001);
        let results: Vec<_> = table
            .closest(&query_id, 2, NonZeroU8::new(1).unwrap())
            .unwrap()
            .into_iter()
            .map(|(sp, contact)| (sp, *contact.id()))
            .collect();
        tracing::trace!("{results:#?}");

        assert_eq!(
            results.len(),
            2,
            "no additional contacts of last bucket, strict XOR-metric"
        );

        assert_eq!(results[0].1, ids[1]);
        assert_eq!(results[0].0.bit_len(), 1);

        assert_eq!(results[1].1, uln);
        assert_eq!(results[1].0.bit_len(), 1);
    }

    #[test]
    #[should_panic]
    fn inner_must_be_empty() {
        let uln = Contact::new(
            Path::from([NodeId::with_msb(2)]),
            SafeStateSeqNr::try_from(1).unwrap(),
        );

        let mut inner = FlatRoutingTable::default(NodeId::ZERO);
        if inner.insert(uln).is_err() {
            return;
        }

        // the newly created UnlimitedULNRoutingTable wouldn't move the existing uln
        // expect a panic

        let _ = UnlimitedULNRoutingTable::from(inner);
    }
    #[test]
    fn delete_uln_from_inner_on_promotion() {
        let mut routing_table =
            UnlimitedULNRoutingTable::from(FlatRoutingTable::default(NodeId::ZERO));

        // node to be promoted to an underlay neighbor shortly
        let node = NodeId::with_msb(2);
        let contact = Contact::new(
            Path::from([NodeId::with_msb(3), node]),
            SafeStateSeqNr::try_from(1).unwrap(),
        );

        let contact_promoted_to_uln =
            Contact::new(Path::from([node]), SafeStateSeqNr::try_from(1).unwrap());

        routing_table
            .insert(contact)
            .expect("failed to insert contact");

        routing_table
            .insert(contact_promoted_to_uln)
            .expect("failed to insert contact");
        assert!(
            !routing_table.inner.contains(&node),
            "promoted underlay neighbor should not be present in inner RT"
        );
    }
}
