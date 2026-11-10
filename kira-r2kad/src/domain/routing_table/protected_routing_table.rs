use std::{
    collections::HashMap,
    num::NonZeroU8,
};

use rand::RngExt as _;
use tracing::Level;

use crate::domain::{
    Bucket,
    Contact,
    ContactLabels,
    ContactState,
    GroupingError,
    NodeId,
    RoutingTable,
    hasher::Hasher,
    routing_table::{
        AddError,
        BucketSplitError,
        FlatRoutingTable,
        PrefixContact,
        ReplacementError,
        sorter_xor,
    },
};

/// A routing table which keeps *protected* contacts in an additional,
/// unlimited data structure next to the regular k-buckets.
///
/// Regular buckets hold at most `BUCKET_SIZE` contacts. Protected contacts are
/// never evicted due to bucket limits. A contact is protected if it has any of
/// the table's protecting [ContactLabels] (by default only [ContactLabels::Uln]).
///
/// In contrast to [FlatRoutingTable] this implementation doesn't replace
/// existing contacts with protected ones.
/// Protected contacts will be added as long as they're not already present in the table.
///
/// # Invariant
///
/// No protected contacts are in the inner routing table.
///
/// # Important
///
/// Calling [`bucket_iter`](ProtectedRoutingTable::bucket_iter) will **not** yield
/// any protected contacts.
#[derive(Debug)]
pub struct ProtectedRoutingTable<const BUCKET_SIZE: usize, const ACC: u8> {
    /// Which of a contact's own labels make this table protect it.
    ///
    /// The labels belong to the [Contact]; this is only the table's policy.
    /// Not every label implies protection (some are only used for filtering).
    protecting_labels: ContactLabels,
    protected_contacts: HashMap<NodeId, Contact>,
    inner: FlatRoutingTable<BUCKET_SIZE, ACC>,
}

impl<const BUCKET_SIZE: usize, const ACC: u8> From<FlatRoutingTable<BUCKET_SIZE, ACC>>
    for ProtectedRoutingTable<BUCKET_SIZE, ACC>
{
    fn from(routing_table: FlatRoutingTable<BUCKET_SIZE, ACC>) -> Self {
        assert!(routing_table.is_empty(), "FlatRoutingTable should be empty");

        Self {
            protecting_labels: ContactLabels::Uln,
            protected_contacts: Default::default(),
            inner: routing_table,
        }
    }
}

impl<const BUCKET_SIZE: usize, const ACC: u8> ProtectedRoutingTable<BUCKET_SIZE, ACC> {
    /// Sets which labels make a contact protected.
    ///
    /// # Panics
    ///
    /// If the table isn't empty, as existing contacts would end up in the wrong place.
    pub fn with_protecting_labels(mut self, labels: ContactLabels) -> Self {
        assert!(
            self.is_empty(),
            "protecting labels can only be changed on an empty table"
        );
        self.protecting_labels = labels;
        self
    }

    pub fn protecting_labels(&self) -> ContactLabels {
        self.protecting_labels
    }

    /// Returns if the contact has to be kept in the protected contacts.
    pub fn is_protected(&self, contact: &Contact) -> bool {
        contact.has_any_label(self.protecting_labels)
    }

    /// Returns if the contact is protected by a stored (local or advertised) label.
    ///
    /// Derived labels like [ContactLabels::Uln] depend on the current path and
    /// are therefore not considered here.
    fn is_protected_by_stored_labels(&self, contact: &Contact) -> bool {
        (contact.labels() - ContactLabels::DERIVED).intersects(self.protecting_labels)
    }
}

impl<'a, const BUCKET_SIZE: usize, const ACC: u8> RoutingTable<'a, BUCKET_SIZE>
    for ProtectedRoutingTable<BUCKET_SIZE, ACC>
{
    type BucketIter = std::slice::Iter<'a, Bucket<BUCKET_SIZE>>;
    type ContactWriteGuard = &'a mut Contact;

    fn root(&self) -> &NodeId {
        self.inner.root()
    }

    fn len(&self) -> usize {
        self.protected_contacts.len() + self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.protected_contacts.is_empty() && self.inner.is_empty()
    }

    fn add(&mut self, mut contact: Contact) -> Result<(), AddError> {
        if let Some(protected_entry) = self.protected_contacts.get(contact.id()) {
            // contact to add exists as protected contact already
            if self.is_protected(&contact)
                || *protected_entry.state() == ContactState::Valid
                || self.is_protected_by_stored_labels(protected_entry)
            {
                return Err(AddError::AlreadyExists(contact.into_id()));
            }
            // the existing contact lost its protection (e.g. a former ULN which lost
            // its direct link), so it moves to the regular buckets
            let previous = self
                .protected_contacts
                .remove(contact.id())
                .expect("presence was checked before");
            contact.inherit_labels_from(&previous);
        } else if self.is_protected(&contact) {
            // contact is not protected yet, but has to be: move it out of the buckets
            if let Some(previous) = self.inner.remove(contact.id()) {
                contact.inherit_labels_from(&previous);
            }
            self.protected_contacts.insert(*contact.id(), contact);
            return Ok(());
        }
        // try regular add, returns own result
        self.inner.add(contact)
    }

    fn remove(&mut self, id: &NodeId) -> Option<Contact> {
        self.protected_contacts
            .remove(id)
            .or_else(|| self.inner.remove(id))
    }

    fn replace(&mut self, id: &NodeId, with: Contact) -> Result<Contact, ReplacementError> {
        if !self.protected_contacts.contains_key(id) {
            return self.inner.replace(id, with);
        }

        if with.id() != id && self.contains(with.id()) {
            return Err(ReplacementError::DuplicateId(*with.id()));
        }

        let old = self
            .protected_contacts
            .remove(id)
            .expect("presence was checked before");
        self.protected_contacts.insert(*with.id(), with);
        Ok(old)
    }

    fn contact(&self, id: &NodeId) -> Option<&Contact> {
        self.protected_contacts
            .get(id)
            .or_else(|| self.inner.contact(id))
    }

    fn random_id(&self) -> Option<&NodeId> {
        if self.is_empty() {
            return None;
        }
        let random = rand::rng().random_range(0..self.len());
        self.iter().nth(random).map(|contact| contact.id())
    }

    fn contact_mut(&'a mut self, id: &NodeId) -> Option<Self::ContactWriteGuard> {
        self.protected_contacts
            .get_mut(id)
            .or_else(|| self.inner.contact_mut(id))
    }

    fn contains(&self, id: &NodeId) -> bool {
        self.protected_contacts.contains_key(id) || self.inner.contains(id)
    }

    fn contains_with<F>(&self, id: &NodeId, f: F) -> bool
    where
        F: Fn(&Contact) -> bool,
    {
        if let Some(contact) = self.protected_contacts.get(id) {
            f(contact)
        } else {
            self.inner.contains_with(id, f)
        }
    }

    fn is_close_contact(&self, id: &NodeId) -> bool {
        self.inner.is_close_contact(id)
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
        target = "routing_table::protected_routing_table",
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
            self.protected_contacts
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
                    target: "routing_table::protected_routing_table",
                    reason = "last bucket contacts require sorting by strict XOR-metric",
                    "Truncate collected results",
                );
                closest.truncate(n);
            }
        }

        Ok(closest)
    }

    fn iter(&self) -> impl Iterator<Item = &Contact> {
        self.protected_contacts.values().chain(self.inner.iter())
    }

    fn iter_mut(&'a mut self) -> impl Iterator<Item = Self::ContactWriteGuard> {
        self.protected_contacts
            .values_mut()
            .chain(self.inner.iter_mut())
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

    use crate::domain::{
        Contact,
        ContactLabels,
        ContactState,
        NodeId,
        NotViaStateList,
        Path,
        RoutingTable,
        SafeStateSeqNr,
        routing_table::{
            FlatRoutingTable,
            ProtectedRoutingTable,
        },
    };

    #[test]
    fn yield_underlay_neighbors() {
        let _clock = crate::tests::TestClock::new();
        let invalid_contact = Contact::new(
            Path::from([NodeId::with_msb(4)]),
            SafeStateSeqNr::try_from(1).unwrap(),
        );

        let mut routing_table =
            ProtectedRoutingTable::from(FlatRoutingTable::default(NodeId::ZERO));

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
        let _clock = crate::tests::TestClock::new();
        let mut invalid_contact = Contact::new(
            Path::from([NodeId::with_msb(4)]),
            SafeStateSeqNr::try_from(1).unwrap(),
        );
        *invalid_contact.state_mut() = ContactState::Invalid(NotViaStateList::default());

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
            ProtectedRoutingTable::from(FlatRoutingTable::default(NodeId::ZERO));
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
        let _clock = crate::tests::TestClock::new();
        let mut invalid_contact = Contact::new(
            Path::from([NodeId::with_msb(2)]),
            SafeStateSeqNr::try_from(1).unwrap(),
        );
        *invalid_contact.state_mut() = ContactState::Invalid(NotViaStateList::default());

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
            ProtectedRoutingTable::from(FlatRoutingTable::default(NodeId::ZERO));
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
        let _clock = crate::tests::TestClock::new();
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
            ProtectedRoutingTable::from(FlatRoutingTable::default(NodeId::ZERO));
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
        let _clock = crate::tests::TestClock::new();
        let mut invalid_neighbor = Contact::new(
            Path::from([NodeId::with_msb(2)]),
            SafeStateSeqNr::try_from(1).unwrap(),
        );
        *invalid_neighbor.state_mut() = ContactState::Invalid(NotViaStateList::default());

        let mut invalid_contact = Contact::new(
            Path::from([NodeId::with_msb(2), NodeId::with_msb(4)]),
            SafeStateSeqNr::try_from(1).unwrap(),
        );
        *invalid_contact.state_mut() = ContactState::Invalid(NotViaStateList::default());

        let contacts = vec![invalid_neighbor, invalid_contact];

        let mut routing_table =
            ProtectedRoutingTable::from(FlatRoutingTable::default(NodeId::ZERO));
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
        let _clock = crate::tests::TestClock::new();
        crate::tests::init();
        let root = NodeId::ZERO;
        let mut table = ProtectedRoutingTable::from(FlatRoutingTable::<1, 1>::new(root).unwrap());

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

        *table.contact_mut(&ids[1]).unwrap().state_mut() =
            ContactState::Invalid(NotViaStateList::default());
        assert_eq!(
            table.contact(&ids[1]).unwrap().state(),
            &ContactState::Invalid(NotViaStateList::default()),
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
        let _clock = crate::tests::TestClock::new();
        crate::tests::init();
        let root = NodeId::ZERO;
        let mut table = ProtectedRoutingTable::from(FlatRoutingTable::<2, 1>::new(root).unwrap());

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

        // the newly created ProtectedRoutingTable wouldn't move the existing uln
        // expect a panic

        let _ = ProtectedRoutingTable::from(inner);
    }
    #[test]
    fn delete_uln_from_inner_on_promotion() {
        let _clock = crate::tests::TestClock::new();
        let mut routing_table =
            ProtectedRoutingTable::from(FlatRoutingTable::default(NodeId::ZERO));

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

    #[test]
    fn replace_inner_contact_does_not_protect_it() {
        let _clock = crate::tests::TestClock::new();
        let mut routing_table =
            ProtectedRoutingTable::from(FlatRoutingTable::default(NodeId::ZERO));

        let node = NodeId::with_msb(2);
        let contact = Contact::new(
            Path::from([NodeId::with_msb(3), node]),
            SafeStateSeqNr::try_from(1).unwrap(),
        );
        routing_table
            .insert(contact)
            .expect("failed to insert contact");

        let other = Contact::new(
            Path::from([NodeId::with_msb(3), NodeId::with_msb(4)]),
            SafeStateSeqNr::try_from(1).unwrap(),
        );
        routing_table
            .replace(&node, other)
            .expect("failed to replace contact");

        assert!(
            routing_table.protected_contacts.is_empty(),
            "replacing an inner contact must not add it to the protected contacts"
        );
        assert_eq!(routing_table.len(), 1);
    }

    #[test]
    fn replace_unknown_contact_fails() {
        let _clock = crate::tests::TestClock::new();
        let mut routing_table =
            ProtectedRoutingTable::from(FlatRoutingTable::default(NodeId::ZERO));

        let contact = Contact::new(
            Path::from([NodeId::with_msb(2)]),
            SafeStateSeqNr::try_from(1).unwrap(),
        );

        assert!(
            routing_table
                .replace(&NodeId::with_msb(5), contact)
                .is_err()
        );
        assert!(routing_table.is_empty(), "nothing should have been added");
    }

    #[test]
    fn replace_protected_contact() {
        let _clock = crate::tests::TestClock::new();
        let mut routing_table =
            ProtectedRoutingTable::from(FlatRoutingTable::default(NodeId::ZERO));

        let uln = NodeId::with_msb(2);
        routing_table
            .insert(Contact::new(
                Path::from([uln]),
                SafeStateSeqNr::try_from(1).unwrap(),
            ))
            .expect("failed to insert contact");

        let new_uln = NodeId::with_msb(5);
        let replaced = routing_table
            .replace(
                &uln,
                Contact::new(Path::from([new_uln]), SafeStateSeqNr::try_from(1).unwrap()),
            )
            .expect("failed to replace contact");

        assert_eq!(replaced.id(), &uln);
        assert!(!routing_table.contains(&uln));
        assert!(routing_table.protected_contacts.contains_key(&new_uln));
        assert_eq!(routing_table.len(), 1);
    }

    #[test]
    fn replace_protected_contact_with_existing_id_fails() {
        let _clock = crate::tests::TestClock::new();
        let mut routing_table =
            ProtectedRoutingTable::from(FlatRoutingTable::default(NodeId::ZERO));

        let uln = NodeId::with_msb(2);
        let other = NodeId::with_msb(5);
        for id in [uln, other] {
            routing_table
                .insert(Contact::new(
                    Path::from([id]),
                    SafeStateSeqNr::try_from(1).unwrap(),
                ))
                .expect("failed to insert contact");
        }

        assert!(
            routing_table
                .replace(
                    &uln,
                    Contact::new(Path::from([other]), SafeStateSeqNr::try_from(1).unwrap()),
                )
                .is_err()
        );
        assert_eq!(routing_table.len(), 2, "no contact should be lost");
    }

    fn contact(path: &[NodeId]) -> Contact {
        Contact::new(
            Path::try_from(path).expect("test path must not be empty"),
            SafeStateSeqNr::try_from(1).unwrap(),
        )
    }

    fn cached(path: &[NodeId]) -> Contact {
        let mut contact = contact(path);
        contact.add_local_labels(ContactLabels::Cached);
        contact
    }

    #[test]
    fn only_ulns_are_protected_by_default() {
        let _clock = crate::tests::TestClock::new();
        let mut routing_table =
            ProtectedRoutingTable::from(FlatRoutingTable::default(NodeId::ZERO));
        assert_eq!(routing_table.protecting_labels(), ContactLabels::Uln);

        let node = NodeId::with_msb(2);
        routing_table
            .insert(cached(&[NodeId::with_msb(3), node]))
            .expect("failed to insert contact");

        assert!(routing_table.inner.contains(&node));
        assert!(routing_table.protected_contacts.is_empty());
    }

    #[test]
    fn contacts_with_protecting_labels_are_protected() {
        let _clock = crate::tests::TestClock::new();
        let mut routing_table =
            ProtectedRoutingTable::from(FlatRoutingTable::default(NodeId::ZERO))
                .with_protecting_labels(ContactLabels::Uln | ContactLabels::Cached);

        let node = NodeId::with_msb(2);
        routing_table
            .insert(cached(&[NodeId::with_msb(3), node]))
            .expect("failed to insert contact");

        assert!(!routing_table.inner.contains(&node));
        assert!(routing_table.protected_contacts.contains_key(&node));
        assert!(
            routing_table
                .bucket_iter()
                .all(|bucket| !bucket.contains(&node)),
            "protected contacts are not in the buckets"
        );
    }

    #[test]
    #[should_panic]
    fn protecting_labels_cannot_change_on_filled_table() {
        let mut routing_table =
            ProtectedRoutingTable::from(FlatRoutingTable::default(NodeId::ZERO));
        routing_table
            .insert(contact(&[NodeId::with_msb(2)]))
            .expect("failed to insert contact");

        let _ = routing_table.with_protecting_labels(ContactLabels::Cached);
    }

    #[test]
    fn promotion_keeps_labels() {
        let _clock = crate::tests::TestClock::new();
        let mut routing_table =
            ProtectedRoutingTable::from(FlatRoutingTable::default(NodeId::ZERO));

        let node = NodeId::with_msb(2);
        let mut stored = cached(&[NodeId::with_msb(3), node]);
        stored.set_advertised_labels(Some(ContactLabels::Landmark));
        routing_table
            .insert(stored)
            .expect("failed to insert contact");

        // node becomes an underlay neighbor
        routing_table
            .insert(contact(&[node]))
            .expect("failed to insert contact");

        assert_eq!(
            routing_table.protected_contacts[&node].labels(),
            ContactLabels::Uln | ContactLabels::Cached | ContactLabels::Landmark
        );
    }

    #[test]
    fn demotion_keeps_labels() {
        let _clock = crate::tests::TestClock::new();
        let mut routing_table =
            ProtectedRoutingTable::from(FlatRoutingTable::default(NodeId::ZERO));

        let node = NodeId::with_msb(2);
        let mut former_uln = cached(&[node]);
        former_uln.set_state(ContactState::Invalid(NotViaStateList::default()));
        routing_table
            .insert(former_uln)
            .expect("failed to insert contact");

        // node is now reachable via a longer path only
        routing_table
            .insert(contact(&[NodeId::with_msb(3), node]))
            .expect("failed to insert contact");

        assert!(routing_table.protected_contacts.is_empty());
        assert_eq!(
            routing_table.inner.contact(&node).unwrap().labels(),
            ContactLabels::Cached
        );
    }

    #[test]
    fn no_demotion_while_protected_by_stored_label() {
        let _clock = crate::tests::TestClock::new();
        let mut routing_table =
            ProtectedRoutingTable::from(FlatRoutingTable::default(NodeId::ZERO))
                .with_protecting_labels(ContactLabels::Uln | ContactLabels::Cached);

        let node = NodeId::with_msb(2);
        let mut former_uln = cached(&[node]);
        former_uln.set_state(ContactState::Invalid(NotViaStateList::default()));
        routing_table
            .insert(former_uln)
            .expect("failed to insert contact");

        assert!(
            routing_table
                .insert(contact(&[NodeId::with_msb(3), node]))
                .is_err(),
            "still cached contact must stay protected"
        );
        assert!(routing_table.protected_contacts.contains_key(&node));
        assert!(!routing_table.inner.contains(&node));
    }

    #[test]
    fn iter_with_any_label() {
        let _clock = crate::tests::TestClock::new();
        let mut routing_table =
            ProtectedRoutingTable::from(FlatRoutingTable::default(NodeId::ZERO));

        let uln = NodeId::with_msb(2);
        let cached_node = NodeId::with_msb(3);
        let plain_node = NodeId::with_msb(4);
        routing_table
            .extend(
                false,
                [
                    contact(&[uln]),
                    cached(&[uln, cached_node]),
                    contact(&[uln, plain_node]),
                ],
            )
            .expect("failed to insert contacts");

        let mut ids: Vec<_> = routing_table
            .iter_with_any_label(ContactLabels::Uln | ContactLabels::Cached)
            .map(|contact| *contact.id())
            .collect();
        ids.sort();
        let mut expected = vec![uln, cached_node];
        expected.sort();
        assert_eq!(ids, expected);
    }
}
