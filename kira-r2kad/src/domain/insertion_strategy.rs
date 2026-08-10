use std::{
    fmt::Debug,
    marker::PhantomData,
    ops::Deref as _,
};

use tracing::{
    Level,
    instrument,
};

use super::{
    PathSimplifier,
    ULNTable,
};
use crate::domain::{
    AddError,
    Contact,
    ContactState,
    InsertionError,
    NodeId,
    PathCycleRemover,
    RoutingTable,
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
    /// FIXME: this should only be done after checking that the contact is currently reachable
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
        // perform some sanity checks

        // valid contacts stem from source route, unknown contacts from RTable objects
        if !(contact.is_valid() || *contact.state() == ContactState::Unknown) {
            tracing::trace!(
                target: "routing_table",
                reason = "tried to insert non valid contact",
                path = %contact.path().unwrap(),
                "dropping contact",
            );
            return InsertionStrategyResult::Dropped;
        }
        // Ignore paths to us
        if contact.id() == routing_table.root() {
            tracing::trace!(
                target: "routing_table",
                reason = "ignoring myself as contact",
                path = %contact.path().unwrap(),
                "dropping contact",
            );
            return InsertionStrategyResult::Dropped;
        }
        // Ignore paths via us or contacts containing our own id
        if contact.path().unwrap().contains(routing_table.root()) {
            tracing::trace!(
                target: "routing_table",
                reason = "path contains myself",
                path = %contact.path().unwrap(),
                "dropping contact",
            );
            return InsertionStrategyResult::Dropped;
        }
        // If the first element is no underlay neighbor
        if !un_table.contains(contact.path().unwrap().first()) {
            tracing::trace!(
                target: "routing_table",
                reason = "path not leading via underlay neighbor",
                node = %contact.path().unwrap().first(),
                path = %contact.path().unwrap(),
                "dropping contact",
            );
            return InsertionStrategyResult::Dropped;
        }

        // remove cycles and simplify
        // Uses the Path containing the id of the contact
        // itself to include it in the process
        let path = contact.path_mut().unwrap();
        self.path_cycle_remover.remove_cycles_in_place(path);
        self.path_simplifier.simplify(routing_table, un_table, path);

        // contacts with Unknown state contain may new interesting paths for proposed paths
        // try them first if contact exists
        if *contact.state() == ContactState::Unknown {
            if routing_table.contains(contact.id()) {
                return self.update_existing(contact.clone(), routing_table);
            }
            // if the contact does not exist yet, return for now
            // TODO interesting contacts could be saved for later
            return InsertionStrategyResult::Dropped;
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

        // try modifying an existing contact in the bucket instead of inserting
        match insertion_err {
            InsertionError::BucketSplit(_) | InsertionError::Add(AddError::NotAdded) => {
                self.replace_in_full_bucket(contact.clone(), routing_table)
            }
            InsertionError::Add(AddError::AlreadyExists(_)) => {
                self.update_existing(contact.clone(), routing_table)
            }
        }
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
