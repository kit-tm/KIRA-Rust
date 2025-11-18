use std::{fmt::Debug, marker::PhantomData, ops::Deref as _};

use tracing::{Level, instrument};

use crate::domain::{
    AddError, Contact, ContactState, InsertionError, NodeId, PathCycleRemover, RoutingTable,
};

use super::{PathSimplifier, ULNTable};

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
            .expect("update_existing should only be called after detecting a id to be present");
        assert_eq!(
            existing.id(),
            contact.id(),
            "Returned contact has to have same id"
        );
        if contact.state() != &ContactState::Valid {
            tracing::trace!(target: "routing_table", "Dropped path: Invalid [{contact:?}]");
            return InsertionStrategyResult::Dropped;
        }

        // drop if received data is older than stored data
        // FIXME: update if on direct contact (self-controlled Nonce) to catch wrap
        if contact.is_older_than(&existing) {
            log::trace!(
                target: "routing_table",
                "Dropping path: Older [info: {:?}, saved_age: {:?}, saved_ssn: {:?}]",
                contact,
                existing.age(),
                existing.state_seq_nr(),
            );
            return InsertionStrategyResult::Dropped;
        }

        // contact is newer, better or fixes a contact

        // replace invalid existing data
        // FIXME: check whether the supposed new path actually avoids all broken links
        // -> `src/routing/r2kademlia/KadRoutingTable.cc:299`
        if existing.state() == &ContactState::Invalid && contact.state() == &ContactState::Valid {
            log::trace!(target: "routing_table", "Updated path: Invalid path was replaced [{contact:?}]");
            *existing = contact;
            return InsertionStrategyResult::Updated;
        }

        // don't replace path with longer path if ssn is same
        if existing.state_seq_nr() == contact.state_seq_nr()
            // TODO: support option to specify replace behaviour on equal length (called `enableSinglePathDiversity`)
            // TODO: use hash to prevent path flapping
            && contact.path().size() >= /* > */ existing.path().size()
        {
            log::trace!(
                target: "routing_table",
                "Not updating contacts path because it is longer [{contact:?}]"
            );
            return InsertionStrategyResult::Dropped;
        }

        // But: If only age is updated, don't emit anything
        let return_result =
            if contact.path() == existing.path() && contact.state() == existing.state() {
                log::trace!(
                    target: "routing_table",
                    "Not updating contacts path because its the same and doesn't change state [{}]",
                    contact.id()
                );

                // WARN: This will update the SSN even if the InsertionStrategyResult is Dropped
                //  be sure to notify other UseCases with UseCaseEvent::Resync
                // TODO: figure out if skipping the update of the SSN causes trouble
                //  this would keep the routing-table in a more sensible state
                //  this would maybe cause delayed UpdateRouteReq,

                existing.set_last_seen_now();
                *existing.state_seq_nr_mut() = *contact.state_seq_nr();
                InsertionStrategyResult::Dropped
            } else {
                // FIXME: Don't accept longer path to potential UN
                //   this potentially also requires to rework the PathSimplifier,
                //   since it just assumes working UN and replaces with existing short path
                // TODO: schedule recheck UN if in vicinityDiscoveryRadius
                // TODO: schedule pathcheck for shorter path if offered path is longer
                // -> src/routing/r2kademlia/KadRoutingTable.cc:315
                log::trace!(
                    target: "routing_table",
                    "Updated contact [{contact:?}]"
                );
                existing.set_last_seen_now();
                *existing = contact.clone();
                InsertionStrategyResult::Updated
            };

        if existing.path().size() > contact.path().size() {
            log::warn!(target: "insertion_strategy", "New Path {:?} is better than existing path {:?}, 
                but InsertionStrategyResult is {:?} ", contact.path(), existing.path(), return_result);
        }

        return_result
    }

    /// Check if the contact can replace an entry in the bucket it belongs to.
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
            .filter(|c| c.path().size() > contact.path().size())
            .max_by_key(|c| c.path().size());

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
        if *contact.state() != ContactState::Valid {
            tracing::trace!(
                target: "routing_table",
                state = %contact.state(),
                reason = "invalid_state",
                path = %contact.path(),
                "dropping contact",
            );
            return InsertionStrategyResult::Dropped;
        }

        // Ignore paths to us
        if contact.id() == routing_table.root() {
            tracing::trace!(
                target: "routing_table",
                reason = "us",
                path = %contact.path(),
                "dropping contact",
            );
            return InsertionStrategyResult::Dropped;
        }
        // Ignore paths via us or contacts containing our own id
        if contact.path().contains(routing_table.root()) {
            tracing::trace!(
                target: "routing_table",
                reason = "via_us",
                path = %contact.path(),
                "dropping contact",
            );
            return InsertionStrategyResult::Dropped;
        }
        // If the first element is no underlay neighbor
        if !un_table.contains(contact.path().first()) {
            tracing::trace!(
                target: "routing_table",
                reason = "no_underlay_neighbor",
                node = %contact.path().first(),
                path = %contact.path(),
                "dropping contact",
            );
            return InsertionStrategyResult::Dropped;
        }

        // remove cycles and simplify
        // Uses the Path containing the id of the contact
        // itself to include it in the process
        let path = contact.path_mut();
        self.path_cycle_remover.remove_cycles_in_place(path);
        self.path_simplifier.simplify(routing_table, un_table, path);

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
        let result = routing_table.insert(contact);
        if let Err(e) = result {
            log::warn!(target: "routing_table", "Failed to insert contact into routing table: {e}");
        }

        self.0.clone()
    }
}
