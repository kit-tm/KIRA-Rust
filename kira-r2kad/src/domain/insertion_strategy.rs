use std::{
    fmt::Debug,
    marker::PhantomData,
    ops::Deref as _,
};

use tracing::{
    Level,
    instrument,
};

use crate::domain::{
    Contact,
    ContactState,
    NodeId,
    RoutingTable,
    ULNTable,
    path::{
        PathCycleRemover,
        PathSimplifier,
    },
    routing_table::{
        AddError,
        InsertionError,
    },
};

/// Signals if a change to the [Path](crate::domain::path::Path) of a contact happened.
///
/// This doesn't address changes to the other fields of the [Contact].
#[derive(Debug, Eq, PartialEq, Clone)]
pub enum InsertionStrategyResult {
    Dropped,
    Inserted,
    Replaced(NodeId),
    Updated,
    NeedsValidation,
}

/// An [InsertionStrategy] handles inserting a [Contact] into a [RoutingTable].
///
/// The actions performed with the [Contact] are limited to the [InsertionStrategyResult].
///
/// The Algorithm can use the [ULNTable] but is not allowed to insert into it.
/// This will be handled where the Hello-Messages are handled explicitly.
///
/// Also [NotVia](crate::domain::NotVia) Data is not handled by the [InsertionStrategy] as it
/// represents logic of the routing-daemon itself and not the domain.
pub trait InsertionStrategy<RT, UN, const BUCKET_SIZE: usize>
where
    for<'a> RT: RoutingTable<'a, BUCKET_SIZE>,
{
    /// Insert the [Contact] into the [RoutingTable].
    ///
    /// Will return the action performed for the [Contact].
    fn insert(
        &mut self,
        contact: Contact,
        routing_table: &mut RT,
        un_table: &UN,
    ) -> InsertionStrategyResult;
}

#[derive(Debug, Default)]
pub struct UNSStrategy<RT, CR, PS, const BUCKET_SIZE: usize> {
    _pd: PhantomData<RT>,
    path_cycle_remover: CR,
    path_simplifier: PS,
}

impl<RT, CR, PS, const BUCKET_SIZE: usize> UNSStrategy<RT, CR, PS, BUCKET_SIZE> {
    pub fn new(path_cycle_remover: CR, path_simplifier: PS) -> Self {
        Self {
            _pd: PhantomData,
            path_cycle_remover,
            path_simplifier,
        }
    }
}

impl<RT, CR, PS, const BUCKET_SIZE: usize> UNSStrategy<RT, CR, PS, BUCKET_SIZE>
where
    for<'a> RT: RoutingTable<'a, BUCKET_SIZE>,
{
    /// Inserts a contact whose path has already been checked, cycle-free and simplified.
    fn insert_prepared(&self, contact: Contact, routing_table: &mut RT) -> InsertionStrategyResult {
        // contacts with Unknown state contain may new interesting paths for proposed paths
        // try them first if contact exists
        if *contact.state() == ContactState::Unknown {
            if routing_table.contains(contact.id()) {
                return self.update_existing(contact.clone(), routing_table);
            }
            // if the contact does not exist yet, return for now
            // interesting contacts could be saved for later
            // TODO need to assess whether it is worth to probe the path
            return InsertionStrategyResult::NeedsValidation;
        }

        // insert modified contact into routing table
        let insertion_result = routing_table.insert(contact.clone());

        let Err(insertion_err) = insertion_result else {
            tracing::debug!(
                target: "insertion_strategy",
                node = %contact.id(),
                %contact,
                "inserted contact into routing table"
            );
            return InsertionStrategyResult::Inserted;
        };
        tracing::trace!(
            target: "insertion_strategy",
            node = %contact.id(),
            %contact,
            error = %insertion_err,
            "inserting contact into routing table failed",
        );

        // try modifying an existing contact in the bucket instead of inserting or check new contact proposal first
        match insertion_err {
            InsertionError::BucketSplit(_) | InsertionError::Add(AddError::NotAdded) => {
                self.replace_in_full_bucket(contact.clone(), routing_table)
            }
            InsertionError::Add(AddError::AlreadyExists(_)) => {
                self.update_existing(contact.clone(), routing_table)
            }
            InsertionError::Add(AddError::NeedsValidation) => {
                return InsertionStrategyResult::NeedsValidation;
            }
        }
    }

    /// Update an existing contact in the table instead of inserting.
    fn update_existing(&self, contact: Contact, table: &mut RT) -> InsertionStrategyResult {
        let mut existing = table
            .contact_mut(contact.id())
            .expect("update_existing should only be called after detecting an id to be present");
        assert_eq!(
            existing.id(),
            contact.id(),
            "Returned contact has to have same id"
        );

        // drop if received data is older than stored data
        // FIXME: update if on direct contact (self-controlled Nonce) to catch wrapping sequence numbers
        if contact.is_older_than(&existing) {
            log::trace!(
                target: "routing_table",
                "Dropping path to {}: Older [info: {:?}, existing_age: {:?}, existing_ssn: {:?}]",
                contact.id(),
                contact,
                existing.age(),
                existing.state_seq_nr(),
            );
            return InsertionStrategyResult::Dropped;
        }

        // received information is not older: take over its labels and a newer sequence
        // number (it only increases here, a lower one is accepted from authoritative
        // answers of the node only)
        existing.update_labels_from(&contact);
        existing.update_state_seq_nr(*contact.state_seq_nr());
        // a message of the node itself (over a validated source route) shows when it was
        // last heard of, relayed information is someone else's observation
        if *contact.state() == ContactState::Valid
            && contact.path().is_some_and(|path| path.is_valid())
        {
            existing.refresh_last_seen(*contact.last_seen());
        }

        // A message that just traversed the active path shows that this path is current.
        // Refreshing its validation lets older failure reports (NotVia) be recognized as
        // outdated.
        if let Some(candidate) = contact.path()
            && candidate.is_valid()
            && let Some(active) = existing.path_mut()
            && active.is_valid()
            && active.is_same_path_as(candidate)
        {
            active.update_last_validated();
        }

        // contact is newer, may provide better path or improves an invalid contact

        // don't replace path with longer path if ssn is same
        // if paths have the same length the XOR metric is used to determine possible replacement
        // if given path is somehow an improvement (contact validity is considered as well) it will be set as new proposed path
        if existing.state_seq_nr() <= contact.state_seq_nr() {
            if existing.assess_path_candidate_and_update(
                contact.path().expect("contact is expected to have a path"),
            ) {
                if contact.path().unwrap().is_valid() {
                    log::trace!(target: "routing_table", "Updated path: active path was replaced by [{:?} ]", contact.path().unwrap());
                } else {
                    log::trace!(target: "routing_table", "Updated path: proposed path was replaced by [{:?} ]", contact.path().unwrap());
                }
                return InsertionStrategyResult::Updated;
            } else {
                log::trace!(
                    target: "routing_table",
                    "Not updating path to contact {}, because it is not shorter/better [{:?}] than existing [{:?}]",
                    contact.id(),
                    contact.path(),
                    existing.path()
                );
                return InsertionStrategyResult::Dropped;
            }
        }

        InsertionStrategyResult::Dropped
    }

    /// Check if the contact can replace an entry in the bucket it belongs to.
    /// Note: this should only be done after checking that the contact is currently reachable
    /// in case the path has not been validated yet
    fn replace_in_full_bucket(&self, contact: Contact, table: &mut RT) -> InsertionStrategyResult {
        let bucket = table.bucket(contact.id());
        assert!(
            !bucket.is_empty(),
            "replace_in_full_bucket is called on empty bucket"
        );
        assert!(
            bucket.is_full(),
            "replace_in_full_bucket is called on non-full bucket"
        );
        assert!(
            !bucket.contains(contact.id()),
            "replace_in_full_bucket is called containing the replacement candidate"
        );
        assert_eq!(
            *contact.state(),
            ContactState::Valid,
            "replace_in_full_bucket is called with invalid contact",
        );

        // Drop contact with the longest path: Proximity Neighbor Selection (PNS)

        // Get the contact with the longest path but only if longer than new contact
        let replaceable = bucket
            .iter()
            .filter(|c| c.path().unwrap().size() > contact.path().unwrap().size())
            .max_by_key(|c| c.path().unwrap().size());

        if let Some(replaceable) = replaceable.cloned() {
            // since contact is a candidate we are interested in,
            // we need to validate it first in case the path has not been validated
            if contact.path().unwrap().is_notvalidated() {
                return InsertionStrategyResult::NeedsValidation;
            }

            // obtain mut reference to replaceable contact
            let mut replaceable_mut = table
                .contact_mut(replaceable.id())
                .expect("contact inside rt bucket");
            *replaceable_mut = contact;

            tracing::debug!(
                target: "routing_table",
                reason = "proximity_neighbor_selection",
                old = %replaceable, // contains original contact
                new = %replaceable_mut.deref(), // contains the new substitute
                node = %replaceable_mut.id(),
                "replaced contact in bucket",
            );
            return InsertionStrategyResult::Replaced(*replaceable.id());
        }

        tracing::debug!(
            target: "routing_table",
            reason = "bucket_full",
            node = %contact,
            "dropped contact",
        );

        InsertionStrategyResult::Dropped
    }
}

impl<RT, UN, CR, PS, const BUCKET_SIZE: usize> InsertionStrategy<RT, UN, BUCKET_SIZE>
    for UNSStrategy<RT, CR, PS, BUCKET_SIZE>
where
    for<'a> RT: RoutingTable<'a, BUCKET_SIZE>,
    UN: ULNTable,
    CR: PathCycleRemover,
    PS: PathSimplifier,
{
    #[instrument(
        level = Level::TRACE,
        target = "insertion_strategy",
        "insert_contact",
        skip_all,
        ret,
        fields(
            self,
            node = %contact.id(),
            %contact,
        )
    )]
    fn insert(
        &mut self,
        mut contact: Contact,
        routing_table: &mut RT,
        un_table: &UN,
    ) -> InsertionStrategyResult {
        // perform some sanity checks before insertion is actually tried:

        // valid contacts stem from source route, contacts in unknown state from RTable objects
        if !(contact.is_valid() || *contact.state() == ContactState::Unknown) {
            tracing::trace!(
                target: "routing_table",
                reason = "tried to insert non valid contact",
                path = %contact.path().unwrap(),
                "ignoring contact",
            );
            return InsertionStrategyResult::Dropped;
        }
        // Ignore paths to us
        if contact.id() == routing_table.root() {
            tracing::trace!(
                target: "routing_table",
                reason = "ignoring myself as contact",
                path = %contact.path().unwrap(),
                "ignoring contact",
            );
            return InsertionStrategyResult::Dropped;
        }
        // Ignore paths via us or contacts containing our own id
        if contact.path().unwrap().contains(routing_table.root()) {
            tracing::trace!(
                target: "routing_table",
                reason = "path contains myself",
                path = %contact.path().unwrap(),
                "ignoring contact",
            );
            return InsertionStrategyResult::Dropped;
        }

        // Ignore the path if the first element is no bidirectional underlay neighbor:
        // a tentative link is only known to work towards us, not in the direction of the path
        if contact.path().unwrap().size() > 1
            && !un_table.is_bidirectional(contact.path().unwrap().first())
        {
            tracing::trace!(
                target: "routing_table",
                reason = "path not leading via bidirectional underlay neighbor",
                node = %contact.path().unwrap().first(),
                path = %contact.path().unwrap(),
                "ignoring contact",
            );
            return InsertionStrategyResult::Dropped;
        }

        // remove cycles and simplify
        // Uses the Path containing the id of the contact
        // itself to include it in the process
        let path = contact.path_mut().unwrap();
        self.path_cycle_remover.remove_cycles_in_place(path);
        // without cycles, the path still only consists of links the message traversed
        let traversed = path.clone();
        self.path_simplifier.simplify(routing_table, un_table, path);
        let simplified = !path.is_same_path_as(&traversed);
        if simplified {
            // the path was improved with local information, so it is not validated
            path.set_notvalidated();
        }
        // if it is a ULN it is automatically a valid path
        if path.size() == 1 {
            path.set_valid();
        }

        // A validated traversed path must not get lost by simplification:
        // use it as candidate and offer the simplified path as proposed path to be probed.
        if simplified && traversed.is_valid() && path.is_notvalidated() {
            let mut validated = contact.clone();
            validated.set_path(traversed);
            let result = self.insert_prepared(validated, routing_table);

            if routing_table.contains(contact.id()) {
                let proposal = self.update_existing(contact, routing_table);
                if result == InsertionStrategyResult::Dropped
                    && proposal == InsertionStrategyResult::Updated
                {
                    return InsertionStrategyResult::Updated;
                }
            }
            return result;
        }

        self.insert_prepared(contact, routing_table)
    }
}

/// Test Implementation for [InsertionStrategy].
///
/// **Should only be used for testing!**.
///
/// Returns the result given on initialization after inserting the [Contact] into the [RoutingTable].
/// Ignores any results while calling [RoutingTable::insert].
pub struct TestInsertionStrategy(InsertionStrategyResult);

impl From<InsertionStrategyResult> for TestInsertionStrategy {
    fn from(result: InsertionStrategyResult) -> Self {
        Self(result)
    }
}

impl<RT, UN, const BUCKET_SIZE: usize> InsertionStrategy<RT, UN, BUCKET_SIZE>
    for TestInsertionStrategy
where
    for<'a> RT: RoutingTable<'a, BUCKET_SIZE>,
{
    fn insert(
        &mut self,
        contact: Contact,
        routing_table: &mut RT,
        _un_table: &UN,
    ) -> InsertionStrategyResult {
        let contact_id = *contact.id();
        let result = routing_table.insert(contact);
        if let Err(e) = result {
            log::warn!(target: "routing_table", "Failed to insert contact {} into routing table: {}", contact_id, e);
        }

        self.0.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        ContactLabels,
        ContactState,
        NodeId,
        Path,
        SafeStateSeqNr,
        UnderlayNeighborId,
        path::{
            InOrderCycleRemover,
            PathSimplifier,
        },
        routing_table::{
            RoutingTable,
            SingleBucketRT,
        },
        underlay::{
            ConnectionId,
            InterfaceId,
        },
        underlay_neighbor_table::InMemoryULNTable,
    };

    const BUCKET_SIZE: usize = 4;

    /// A simplifier that never modifies paths, to keep tests deterministic.
    struct NoopSimplifier;

    impl PathSimplifier for NoopSimplifier {
        fn simplify<RT, UN, const BS: usize>(&mut self, _: &RT, _: &UN, _: &mut Path)
        where
            for<'a> RT: RoutingTable<'a, BS>,
            UN: ULNTable,
        {
        }
    }

    fn ulnid() -> UnderlayNeighborId {
        UnderlayNeighborId {
            interface_id: InterfaceId::try_from(1).unwrap(),
            connection_id: ConnectionId::from(1),
        }
    }

    /// ULN table with tentative entries (only heard from, link not confirmed).
    fn tentative_uln_table_with(ids: &[NodeId]) -> InMemoryULNTable {
        let mut table = InMemoryULNTable::new();
        for id in ids {
            table.insert(*id, ulnid());
        }
        table
    }

    /// ULN table with bidirectional entries.
    fn uln_table_with(ids: &[NodeId]) -> InMemoryULNTable {
        let mut table = tentative_uln_table_with(ids);
        for id in ids {
            table.confirm(*id, ulnid());
        }
        table
    }

    fn strategy()
    -> UNSStrategy<SingleBucketRT<BUCKET_SIZE>, InOrderCycleRemover, NoopSimplifier, BUCKET_SIZE>
    {
        UNSStrategy::new(InOrderCycleRemover, NoopSimplifier)
    }

    fn contact(path: &[NodeId], ssn: u32) -> Contact {
        Contact::new(
            Path::try_from(path).expect("test path must not be empty"),
            SafeStateSeqNr::try_from(ssn).expect("test ssn must be valid"),
        )
    }

    fn unknown_contact(path: &[NodeId], ssn: u32) -> Contact {
        let mut contact = contact(path, ssn);
        contact.set_state(ContactState::Unknown);
        contact
    }

    #[test]
    fn inserts_valid_new_contact() {
        let _clock = crate::tests::TestClock::new();
        crate::tests::init();
        let root = NodeId::ZERO;
        let uln = NodeId::from(1);
        let dest = NodeId::from(2);
        let mut rt = SingleBucketRT::<BUCKET_SIZE>::new(root);
        let un = uln_table_with(&[uln]);
        let mut strategy = strategy();

        let result = strategy.insert(contact(&[uln, dest], 1), &mut rt, &un);

        assert_eq!(result, InsertionStrategyResult::Inserted);
        assert!(rt.contains(&dest));
    }

    #[test]
    fn drops_contact_with_root_id() {
        let _clock = crate::tests::TestClock::new();
        crate::tests::init();
        let root = NodeId::ZERO;
        let uln = NodeId::from(1);
        let mut rt = SingleBucketRT::<BUCKET_SIZE>::new(root);
        let un = uln_table_with(&[uln]);
        let mut strategy = strategy();

        let result = strategy.insert(contact(&[uln, root], 1), &mut rt, &un);

        assert_eq!(result, InsertionStrategyResult::Dropped);
        assert!(rt.is_empty() || !rt.contains(&root));
    }

    #[test]
    fn drops_path_containing_root() {
        let _clock = crate::tests::TestClock::new();
        crate::tests::init();
        let root = NodeId::ZERO;
        let uln = NodeId::from(1);
        let dest = NodeId::from(2);
        let mut rt = SingleBucketRT::<BUCKET_SIZE>::new(root);
        let un = uln_table_with(&[uln]);
        let mut strategy = strategy();

        // path [uln, root, dest] routes via ourselves
        let result = strategy.insert(contact(&[uln, root, dest], 1), &mut rt, &un);

        assert_eq!(result, InsertionStrategyResult::Dropped);
        assert!(!rt.contains(&dest));
    }

    #[test]
    fn drops_path_not_starting_at_underlay_neighbor() {
        let _clock = crate::tests::TestClock::new();
        crate::tests::init();
        let root = NodeId::ZERO;
        let uln = NodeId::from(1);
        let foreign = NodeId::from(7);
        let dest = NodeId::from(2);
        let mut rt = SingleBucketRT::<BUCKET_SIZE>::new(root);
        let un = uln_table_with(&[uln]);
        let mut strategy = strategy();

        // first hop `foreign` is not an underlay neighbor
        let result = strategy.insert(contact(&[foreign, dest], 1), &mut rt, &un);

        assert_eq!(result, InsertionStrategyResult::Dropped);
        assert!(!rt.contains(&dest));
    }

    #[test]
    fn unknown_contact_for_new_id_needs_validation() {
        let _clock = crate::tests::TestClock::new();
        crate::tests::init();
        let root = NodeId::ZERO;
        let uln = NodeId::from(1);
        let dest = NodeId::from(2);
        let mut rt = SingleBucketRT::<BUCKET_SIZE>::new(root);
        let un = uln_table_with(&[uln]);
        let mut strategy = strategy();

        let result = strategy.insert(unknown_contact(&[uln, dest], 1), &mut rt, &un);

        assert_eq!(result, InsertionStrategyResult::NeedsValidation);
        assert!(!rt.contains(&dest));
    }

    #[test]
    fn unknown_contact_updates_existing() {
        let _clock = crate::tests::TestClock::new();
        crate::tests::init();
        let root = NodeId::ZERO;
        let uln = NodeId::from(1);
        let dest = NodeId::from(2);
        let mut rt = SingleBucketRT::<BUCKET_SIZE>::new(root);
        let un = uln_table_with(&[uln]);
        let mut strategy = strategy();

        // establish existing contact with a longer path
        assert_eq!(
            strategy.insert(contact(&[uln, NodeId::from(9), dest], 1), &mut rt, &un),
            InsertionStrategyResult::Inserted
        );

        // shorter candidate with same SSN, but only known via an RTable object
        let result = strategy.insert(unknown_contact(&[uln, dest], 1), &mut rt, &un);

        assert_eq!(result, InsertionStrategyResult::Updated);
        let updated = rt.contact(&dest).expect("contact must exist");
        assert_eq!(
            updated.proposed_path().map(Path::last),
            Some(&dest),
            "candidate must become the proposed path"
        );
        assert_eq!(
            updated.path().expect("active path").size(),
            3,
            "active path must be unchanged for unvalidated candidate"
        );
    }

    #[test]
    fn update_existing_drops_older_state_seq_nr() {
        let _clock = crate::tests::TestClock::new();
        crate::tests::init();
        let root = NodeId::ZERO;
        let uln = NodeId::from(1);
        let dest = NodeId::from(2);
        let mut rt = SingleBucketRT::<BUCKET_SIZE>::new(root);
        let un = uln_table_with(&[uln]);
        let mut strategy = strategy();

        assert_eq!(
            strategy.insert(contact(&[uln, dest], 5), &mut rt, &un),
            InsertionStrategyResult::Inserted
        );

        // candidate with older SSN must be dropped even with a shorter path
        let result = strategy.insert(contact(&[uln, NodeId::from(9), dest], 3), &mut rt, &un);

        assert_eq!(result, InsertionStrategyResult::Dropped);
        assert_eq!(
            rt.contact(&dest).expect("contact").state_seq_nr(),
            &SafeStateSeqNr::try_from(5).unwrap(),
            "stored SSN must be unchanged"
        );
    }

    #[test]
    fn update_existing_drops_longer_path_with_same_ssn() {
        let _clock = crate::tests::TestClock::new();
        crate::tests::init();
        let root = NodeId::ZERO;
        let uln = NodeId::from(1);
        let dest = NodeId::from(2);
        let mut rt = SingleBucketRT::<BUCKET_SIZE>::new(root);
        let un = uln_table_with(&[uln]);
        let mut strategy = strategy();

        assert_eq!(
            strategy.insert(contact(&[uln, dest], 5), &mut rt, &un),
            InsertionStrategyResult::Inserted
        );

        let result = strategy.insert(contact(&[uln, NodeId::from(9), dest], 5), &mut rt, &un);

        assert_eq!(result, InsertionStrategyResult::Dropped);
        assert_eq!(
            rt.contact(&dest).expect("contact").path().map(Path::size),
            Some(2),
            "active path must be unchanged"
        );
    }

    #[test]
    fn update_existing_updates_better_path_with_same_ssn() {
        let _clock = crate::tests::TestClock::new();
        crate::tests::init();
        let root = NodeId::ZERO;
        let uln = NodeId::from(1);
        let dest = NodeId::from(2);
        let mut rt = SingleBucketRT::<BUCKET_SIZE>::new(root);
        let un = uln_table_with(&[uln]);
        let mut strategy = strategy();

        assert_eq!(
            strategy.insert(contact(&[uln, NodeId::from(9), dest], 5), &mut rt, &un),
            InsertionStrategyResult::Inserted
        );

        let result = strategy.insert(contact(&[uln, dest], 5), &mut rt, &un);

        assert_eq!(result, InsertionStrategyResult::Updated);
    }

    #[test]
    fn full_bucket_replaces_contact_with_longest_path() {
        let _clock = crate::tests::TestClock::new();
        crate::tests::init();
        let root = NodeId::ZERO;
        let uln = NodeId::from(1);
        let mut rt = SingleBucketRT::<BUCKET_SIZE>::new(root);
        let un = uln_table_with(&[uln]);
        let mut strategy = strategy();

        // fill the single bucket with contacts having different path lengths
        for (i, path_len) in [3usize, 2, 2, 2].into_iter().enumerate() {
            let mut path = vec![uln];
            for hop in 2..path_len {
                path.push(NodeId::from(
                    100 + u128::try_from(i).unwrap() * 10 + u128::try_from(hop).unwrap(),
                ));
            }
            let dest = NodeId::from(10 + u128::try_from(i).unwrap());
            path.push(dest);
            assert_eq!(
                strategy.insert(contact(&path, 1), &mut rt, &un),
                InsertionStrategyResult::Inserted,
                "contact {i} must be inserted"
            );
        }
        assert_eq!(rt.bucket(&NodeId::from(99)).len(), BUCKET_SIZE);

        // candidate with the shortest path must replace the longest-path contact
        let result = strategy.insert(contact(&[uln, NodeId::from(99)], 1), &mut rt, &un);

        let InsertionStrategyResult::Replaced(replaced) = result else {
            panic!("expected Replaced, got {result:?}");
        };
        assert_eq!(replaced, NodeId::from(10), "longest-path contact replaced");
        assert!(rt.contains(&NodeId::from(99)));
        assert!(!rt.contains(&replaced));
    }

    #[test]
    fn full_bucket_drops_contact_without_shorter_path() {
        let _clock = crate::tests::TestClock::new();
        crate::tests::init();
        let root = NodeId::ZERO;
        let uln = NodeId::from(1);
        let mut rt = SingleBucketRT::<BUCKET_SIZE>::new(root);
        let un = uln_table_with(&[uln]);
        let mut strategy = strategy();

        // fill the bucket exclusively with single-hop paths
        for i in 0..BUCKET_SIZE {
            let dest = NodeId::from(10 + u128::try_from(i).unwrap());
            assert_eq!(
                strategy.insert(contact(&[uln, dest], 1), &mut rt, &un),
                InsertionStrategyResult::Inserted
            );
        }

        // candidate path is not shorter than any stored path
        let result = strategy.insert(contact(&[uln, NodeId::from(99)], 1), &mut rt, &un);

        assert_eq!(result, InsertionStrategyResult::Dropped);
        assert!(!rt.contains(&NodeId::from(99)));
    }

    #[test]
    fn update_existing_takes_labels_only_from_newer_info() {
        let _clock = crate::tests::TestClock::new();
        crate::tests::init();
        let root = NodeId::ZERO;
        let uln = NodeId::from(1);
        let dest = NodeId::from(2);
        let mut rt = SingleBucketRT::<BUCKET_SIZE>::new(root);
        let un = uln_table_with(&[uln]);
        let mut strategy = strategy();

        strategy.insert(contact(&[uln, dest], 2), &mut rt, &un);

        // older information must not change the labels
        let mut older = contact(&[uln, dest], 1);
        older.set_advertised_labels(Some(ContactLabels::EndSystem));
        strategy.insert(older, &mut rt, &un);
        assert_eq!(rt.contact(&dest).unwrap().labels(), ContactLabels::empty());

        // newer information updates the labels, even if the path isn't better
        let mut newer = contact(&[uln, dest], 3);
        newer.set_advertised_labels(Some(ContactLabels::Landmark));
        strategy.insert(newer, &mut rt, &un);
        assert_eq!(rt.contact(&dest).unwrap().labels(), ContactLabels::Landmark);
    }

    #[test]
    fn drops_path_via_tentative_neighbor() {
        let _clock = crate::tests::TestClock::new();
        crate::tests::init();
        let root = NodeId::ZERO;
        let uln = NodeId::from(1);
        let dest = NodeId::from(2);
        let mut rt = SingleBucketRT::<BUCKET_SIZE>::new(root);
        let un = tentative_uln_table_with(&[uln]);
        let mut strategy = strategy();

        let result = strategy.insert(contact(&[uln, dest], 1), &mut rt, &un);

        assert_eq!(result, InsertionStrategyResult::Dropped);
        assert!(!rt.contains(&dest));
    }

    fn ids(path: &Path) -> Vec<NodeId> {
        path.clone().into_iter().collect()
    }

    fn valid_contact(path: &[NodeId], ssn: u32) -> Contact {
        let mut contact = contact(path, ssn);
        contact.path_mut().unwrap().set_valid();
        contact
    }

    #[test]
    fn simplified_path_of_validated_contact_is_only_proposed() {
        let _clock = crate::tests::TestClock::new();
        crate::tests::init();
        let root = NodeId::ZERO;
        let (p, q, m, z) = (
            NodeId::from(1),
            NodeId::from(2),
            NodeId::from(3),
            NodeId::from(4),
        );
        let b = NodeId::from(5);
        let mut rt = SingleBucketRT::<BUCKET_SIZE>::new(root);
        let un = uln_table_with(&[p, b]);
        // known valid contact M via <B, M>
        rt.insert(valid_contact(&[b, m], 1)).unwrap();
        let mut strategy = UNSStrategy::<_, _, _, BUCKET_SIZE>::new(
            InOrderCycleRemover,
            crate::domain::path::ShortestFirstPathSimplifier,
        );

        // traversed <P, Q, M, Z> can be improved with local information to <B, M, Z>
        let result = strategy.insert(valid_contact(&[p, q, m, z], 1), &mut rt, &un);

        assert_eq!(result, InsertionStrategyResult::Inserted);
        let contact = rt.contact(&z).unwrap();
        assert_eq!(ids(contact.path().unwrap()), [p, q, m, z]);
        assert!(contact.path().unwrap().is_valid());
        let proposed = contact.proposed_path().expect("improved path is proposed");
        assert_eq!(ids(proposed), [b, m, z]);
        assert!(proposed.is_notvalidated());
    }

    #[test]
    fn shortcut_to_bidirectional_uln_itself_is_valid() {
        let _clock = crate::tests::TestClock::new();
        crate::tests::init();
        let root = NodeId::ZERO;
        let (p, q, b) = (NodeId::from(1), NodeId::from(2), NodeId::from(3));
        let mut rt = SingleBucketRT::<BUCKET_SIZE>::new(root);
        let un = uln_table_with(&[p, b]);
        let mut strategy = UNSStrategy::<_, _, _, BUCKET_SIZE>::new(
            InOrderCycleRemover,
            crate::domain::path::ShortestFirstPathSimplifier,
        );

        // <P, Q, B> collapses to the direct link <B>, which ULN discovery confirmed
        let result = strategy.insert(valid_contact(&[p, q, b], 1), &mut rt, &un);

        assert_eq!(result, InsertionStrategyResult::Inserted);
        let contact = rt.contact(&b).unwrap();
        assert_eq!(ids(contact.path().unwrap()), [b]);
        assert!(contact.path().unwrap().is_valid());
        assert!(contact.proposed_path().is_none());
    }

    #[test]
    fn unchanged_validated_path_is_inserted_as_is() {
        let _clock = crate::tests::TestClock::new();
        crate::tests::init();
        let root = NodeId::ZERO;
        let (p, q) = (NodeId::from(1), NodeId::from(2));
        let mut rt = SingleBucketRT::<BUCKET_SIZE>::new(root);
        let un = uln_table_with(&[p]);
        let mut strategy = UNSStrategy::<_, _, _, BUCKET_SIZE>::new(
            InOrderCycleRemover,
            crate::domain::path::ShortestFirstPathSimplifier,
        );

        let result = strategy.insert(valid_contact(&[p, q], 1), &mut rt, &un);

        assert_eq!(result, InsertionStrategyResult::Inserted);
        let contact = rt.contact(&q).unwrap();
        assert!(contact.path().unwrap().is_valid());
        assert!(contact.proposed_path().is_none());
    }

    fn last_validated(rt: &SingleBucketRT<BUCKET_SIZE>, id: &NodeId) -> crate::domain::Timestamp {
        *rt.contact(id)
            .unwrap()
            .path()
            .unwrap()
            .get_last_validated()
            .expect("validated path")
    }

    #[test]
    fn traversing_the_active_path_refreshes_its_validation() {
        let mut clock = crate::tests::TestClock::new();
        crate::tests::init();
        let (uln, dest) = (NodeId::from(1), NodeId::from(2));
        let mut rt = SingleBucketRT::<BUCKET_SIZE>::new(NodeId::ZERO);
        let un = uln_table_with(&[uln]);
        let mut strategy = strategy();
        strategy.insert(valid_contact(&[uln, dest], 1), &mut rt, &un);
        let first = last_validated(&rt, &dest);

        clock.advance(std::time::Duration::from_millis(2));
        strategy.insert(valid_contact(&[uln, dest], 1), &mut rt, &un);

        assert!(last_validated(&rt, &dest) > first);
    }

    #[test]
    fn unvalidated_information_does_not_refresh_validation() {
        let mut clock = crate::tests::TestClock::new();
        crate::tests::init();
        let (uln, dest) = (NodeId::from(1), NodeId::from(2));
        let mut rt = SingleBucketRT::<BUCKET_SIZE>::new(NodeId::ZERO);
        let un = uln_table_with(&[uln]);
        let mut strategy = strategy();
        strategy.insert(valid_contact(&[uln, dest], 1), &mut rt, &un);
        let first = last_validated(&rt, &dest);

        clock.advance(std::time::Duration::from_millis(2));
        // e.g. the same path reported in an RTable object
        strategy.insert(unknown_contact(&[uln, dest], 2), &mut rt, &un);

        assert_eq!(last_validated(&rt, &dest), first);
    }

    #[test]
    fn newer_information_raises_the_stored_ssn() {
        let _clock = crate::tests::TestClock::new();
        crate::tests::init();
        let (uln, dest) = (NodeId::from(1), NodeId::from(2));
        let mut rt = SingleBucketRT::<BUCKET_SIZE>::new(NodeId::ZERO);
        let un = uln_table_with(&[uln]);
        let mut strategy = strategy();
        strategy.insert(valid_contact(&[uln, dest], 1), &mut rt, &un);

        // relayed by another node (RTable object), not better, but newer
        strategy.insert(
            unknown_contact(&[uln, NodeId::from(9), dest], 5),
            &mut rt,
            &un,
        );

        assert_eq!(
            rt.contact(&dest).unwrap().state_seq_nr(),
            &SafeStateSeqNr::try_from(5).unwrap()
        );
    }

    #[test]
    fn only_first_hand_messages_refresh_last_seen() {
        let mut clock = crate::tests::TestClock::new();
        crate::tests::init();
        let (uln, dest) = (NodeId::from(1), NodeId::from(2));
        let mut rt = SingleBucketRT::<BUCKET_SIZE>::new(NodeId::ZERO);
        let un = uln_table_with(&[uln]);
        let mut strategy = strategy();
        strategy.insert(valid_contact(&[uln, dest], 1), &mut rt, &un);
        let first = *rt.contact(&dest).unwrap().last_seen();

        clock.advance(std::time::Duration::from_millis(2));
        strategy.insert(unknown_contact(&[uln, dest], 2), &mut rt, &un);
        assert_eq!(*rt.contact(&dest).unwrap().last_seen(), first, "relayed");

        clock.advance(std::time::Duration::from_millis(2));
        // a message of the node over another path
        strategy.insert(
            valid_contact(&[uln, NodeId::from(9), dest], 2),
            &mut rt,
            &un,
        );
        assert!(
            *rt.contact(&dest).unwrap().last_seen() > first,
            "first-hand"
        );
    }
}
