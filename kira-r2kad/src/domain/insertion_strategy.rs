use std::fmt::Debug;
use std::marker::PhantomData;

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
        let existing = table.contact_mut(contact.id());
        assert!(
            existing.is_some(),
            "update_existing should only be called after detecting a id to be present"
        );
        let mut existing = existing.unwrap();
        assert_eq!(
            existing.id(),
            contact.id(),
            "Returned contact has to have same id"
        );
        if contact.state() != &ContactState::Valid {
            log::trace!(target: "routing_table", "Dropped path: Invalid [{contact:?}]");
            return InsertionStrategyResult::Dropped;
        }

        // drop if received data is older than stored data
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
        let mut bucket = table.bucket_mut(contact.id());
        assert!(
            !bucket.is_empty(),
            "replace_in_full_bucket is called on empty bucket"
        );
        assert!(
            bucket.is_full(),
            "replace_in_full_bucket is called on non-full bucket"
        );

        // Get the contact with the longest path but only if longer than new contact
        let replaceable = bucket
            .iter_mut()
            .filter(|c| c.path().size() > contact.path().size())
            // Invariant: acc contains the contact with the longest path in the bucket after processing the first entry
            .max_by_key(|c| c.path().size());

        if let Some(replaceable) = replaceable {
            let old_id = *replaceable.id();
            *replaceable = contact;
            log::debug!(
                target: "routing_table",
                "Replaced {} with {}",
                old_id, replaceable.id()
            );
            return InsertionStrategyResult::Replaced(old_id);
        }

        log::trace!(
            target: "routing_table",
            "Dropped new contact: nothing to replace with [{}]",
            contact.id()
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
    fn insert(
        &mut self,
        mut contact: Contact,
        routing_table: &mut RT,
        un_table: &UN,
    ) -> InsertionStrategyResult {
        // Ignore paths to us
        if contact.id() == routing_table.root() {
            log::trace!(
                target: "routing_table",
                "Dropping contact info: is us {}",
                contact.path()
            );
            return InsertionStrategyResult::Dropped;
        }
        // Ignore paths via us or contacts containing our own id
        if contact.path().contains(routing_table.root()) {
            log::trace!(
                target: "routing_table",
                "Dropping contact info: via us {}",
                contact.path()
            );
            return InsertionStrategyResult::Dropped;
        }
        // If the first element is no underlay neighbor
        if !un_table.contains(contact.path().first()) {
            log::warn!(
                target: "routing_table",
                "Dropping contact info: first element not a underlay neighbor [{contact}]"
            );
            return InsertionStrategyResult::Dropped;
        }

        // remove cycles and simplify
        // Uses the Path containing the id of the contact
        // itself to include it in the process
        let mut path = contact.path().clone();
        self.path_cycle_remover.remove_cycles_in_place(&mut path);
        self.path_simplifier
            .simplify(routing_table, un_table, &mut path);
        *contact.path_mut() = path;

        match routing_table.insert(contact.clone()) {
            Err(InsertionError::BucketSplit(_)) => {
                let result = self.replace_in_full_bucket(contact.clone(), routing_table);
                tracing::debug!(target: "insertion_strategy", ?result, "Replaced in full bucket [{:?}]", contact.path());
                result
            }
            Err(InsertionError::Add(AddError::AlreadyExists(_))) => {
                let result = self.update_existing(contact.clone(), routing_table);
                tracing::debug!(target: "insertion_strategy", ?result, "Updated existing [{:?}]", contact.path());
                result
            }
            Err(InsertionError::Add(AddError::NotAdded)) => {
                let result = self.replace_in_full_bucket(contact.clone(), routing_table);
                tracing::debug!(target: "insertion_strategy", ?result, "Tried to add first, then replaced in full bucket [{:?}]", contact.path());
                result
            }
            Ok(_) => {
                tracing::debug!(target: "insertion_strategy", "Inserted [{:?}]", contact.path());
                InsertionStrategyResult::Inserted
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
