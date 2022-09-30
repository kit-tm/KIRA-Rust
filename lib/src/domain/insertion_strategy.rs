use std::marker::PhantomData;

use crate::domain::{
    AddError, Contact, ContactState, InsertionError, NodeId, PathCycleRemover, RoutingTable,
};

use super::{PNTable, PathSimplifier};

/// Signals if a change to the [Path] of a contact happened.
///
/// This doesn't address changes to the other fields of the [Contact].
#[derive(Debug, Eq, PartialEq, Clone)]
pub enum InsertionStrategyResult {
    Dropped,
    Inserted,
    Replaced(NodeId),
    Updated,
}

/// An [InsertionResult] handles inserting a [Contact] into a [RoutingTable].
///
/// The actions performed with the [Contact] are limited to the [InsertionStrategyResult].
///
/// The Algorithm can use the [PNTable] but is not allowed to insert into it.
/// This will be handled where the Hello-Messages are handled explicitly.
pub trait InsertionStrategy<RT, const BUCKET_SIZE: usize>
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
        pn_table: &PNTable,
    ) -> InsertionStrategyResult;
}

#[derive(Debug, Default)]
pub struct PNSStrategy<RT, CR, PS, const BUCKET_SIZE: usize> {
    _pd: PhantomData<RT>,
    path_cycle_remover: CR,
    path_simplifier: PS,
}

impl<RT, CR, PS, const BUCKET_SIZE: usize> PNSStrategy<RT, CR, PS, BUCKET_SIZE> {
    pub fn new(path_cycle_remover: CR, path_simplifier: PS) -> Self {
        Self {
            _pd: PhantomData::default(),
            path_cycle_remover,
            path_simplifier,
        }
    }
}

impl<RT, CR, PS, const BUCKET_SIZE: usize> PNSStrategy<RT, CR, PS, BUCKET_SIZE>
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

        // drop if received data is older than stored data
        if contact.state_seq_nr() < existing.state_seq_nr() {
            log::trace!(
                target: "routing_table",
                "Dropping path: Lower StateSeqNr [{}]",
                existing.id()
            );
            return InsertionStrategyResult::Dropped;
        }
        // Drop if path is longer
        if contact.path().size() > existing.path().size() {
            log::trace!(
                target: "routing_table",
                "Dropping path: path longer than existing [{}]",
                existing.id()
            );
            return InsertionStrategyResult::Dropped;
        }

        // Replace if seq_nr is greater (newer)
        if contact.state_seq_nr() > existing.state_seq_nr() {
            *existing = contact;
            log::trace!(
                target: "routing_table",
                "Updated path: Greater StateSeqNr [{:?}]",
                *existing
            );
            return InsertionStrategyResult::Updated;
        }

        // Otherwise the seq_nr is equal

        // Drop if seq_nr is equal and contact is not valid
        if existing.state() != &ContactState::Valid {
            log::trace!(
                target: "routing_table",
                "Dropping path: Same StateSeqNr but contact is invalid [{}]",
                existing.id()
            );
            return InsertionStrategyResult::Dropped;
        }
        // Drop if same seq_nr but Age is older
        if contact.age() > existing.age() {
            log::trace!(
                target: "routing_table",
                "Dropping older contact info: {:?}, Existing: {:?} [{}]",
                contact.age(),
                existing.age(),
                contact.id()
            );
            return InsertionStrategyResult::Dropped;
        }
        *existing.last_seen_mut() = contact.last_seen().clone();
        log::trace!(
                    target: "routing_table",
            "Updated age of contact to {:?} [{}]",
            existing.last_seen(),
            existing.id()
        );

        // Drop if state is not valid and the new info doesn't avoid
        // all failed links
        if let ContactState::Rediscovering(rds) = existing.state() {
            for link in &rds.failed_link_list {
                if contact.path().contains_link(link) {
                    log::trace!(
                        target: "routing_table",
                        "Dropping path: via failed link [{}]",
                        existing.id()
                    );
                    return InsertionStrategyResult::Dropped;
                }
            }
        }

        // Finally: contact is newer, better or fixes a contact

        // But: If only age is updated, don't emit anything
        let return_result = match contact.path() == existing.path() {
            true => {
                log::debug!(
                    target: "routing_table",
                    "Not updating contacts path because its the same [{}]",
                    contact.id()
                );
                InsertionStrategyResult::Dropped
            }
            false => {
                log::debug!(
                    target: "routing_table",
                    "Updating contacts path [{}]",
                    existing.id()
                );
                InsertionStrategyResult::Updated
            }
        };

        *existing = contact;

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

        // Get the contact with the longest path
        let replaceable = bucket
            .iter_mut()
            // Invariant: acc contains the contact with the longest path in the bucket after processing the first entry
            .fold(Option::<&mut Contact>::None, |acc, contact| {
                if acc.is_none() || acc.as_ref().unwrap().path().size() < contact.path().size() {
                    Some(contact)
                } else {
                    acc
                }
            });

        if let Some(replaceable) = replaceable {
            let old_id = replaceable.id().clone();
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

impl<RT, CR, PS, const BUCKET_SIZE: usize> InsertionStrategy<RT, BUCKET_SIZE>
    for PNSStrategy<RT, CR, PS, BUCKET_SIZE>
where
    for<'a> RT: RoutingTable<'a, BUCKET_SIZE>,
    CR: PathCycleRemover,
    PS: PathSimplifier,
{
    fn insert(
        &mut self,
        mut contact: Contact,
        routing_table: &mut RT,
        pn_table: &PNTable,
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
        // If the first element is no physical neighbor
        if !pn_table.contains(contact.path().first()) {
            log::trace!(
                target: "routing_table",
                "Dropping contact info: first element not a physical neighbor [{}]",
                contact
            );
            return InsertionStrategyResult::Dropped;
        }

        // remove cycles and simplify
        // Uses the Path containing the id of the contact
        // itself to include it in the process
        let mut path = contact.path().clone();
        self.path_cycle_remover.remove_cycles_in_place(&mut path);
        self.path_simplifier
            .simplify(routing_table, pn_table, &mut path);
        *contact.path_mut() = path;

        match routing_table.insert(contact.clone()) {
            Err(InsertionError::BucketSplit(_)) => {
                self.replace_in_full_bucket(contact, routing_table)
            }
            Err(InsertionError::Add(AddError::AlreadyExists(_))) => {
                self.update_existing(contact, routing_table)
            }
            Err(InsertionError::Add(AddError::NotAdded)) => {
                self.replace_in_full_bucket(contact, routing_table)
            }
            Ok(()) => {
                log::debug!(
                    target: "routing_table",
                    "Inserted contact [{:?}]",
                    contact
                );
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

impl<RT, const BUCKET_SIZE: usize> InsertionStrategy<RT, BUCKET_SIZE> for TestInsertionStrategy
where
    for<'a> RT: RoutingTable<'a, BUCKET_SIZE>,
{
    fn insert(
        &mut self,
        contact: Contact,
        routing_table: &mut RT,
        _pn_table: &PNTable,
    ) -> InsertionStrategyResult {
        let result = routing_table.insert(contact);
        if let Err(e) = result {
            log::warn!(target: "routing_table", "Failed to insert contact into routing table: {}", e);
        }

        self.0.clone()
    }
}
