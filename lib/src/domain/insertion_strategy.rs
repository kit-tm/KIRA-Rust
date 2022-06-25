use std::marker::PhantomData;

use crate::domain::{AddError, Contact, InsertionError, NodeId, RoutingTable, State};

use super::{NeighborTable, PathSimplifier, Port};

#[derive(Debug, Eq, PartialEq)]
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
/// The Algorithm can use the [NeighborTable] but is not allowed to insert into it.
/// This will be handled where the Hello-Messages are handled explicitly.
pub trait InsertionStrategy<RT, NT, const BUCKET_SIZE: usize>
where
    RT: RoutingTable<BUCKET_SIZE>,
    for<'a> &'a RT: IntoIterator<Item = &'a Contact>,
    NT: NeighborTable,
    for<'b> &'b NT: IntoIterator<Item = (&'b NodeId, &'b Port)>,
{
    /// Insert the [Contact] into the [RoutingTable].
    ///
    /// Will return the action performed for the [Contact].
    fn insert(
        &mut self,
        contact: Contact,
        routing_table: &mut RT,
        neighbor_table: &NT,
    ) -> InsertionStrategyResult;
}

pub struct PNSStrategy<RT, const BUCKET_SIZE: usize> {
    _pd: PhantomData<RT>,
}

impl<RT, const BUCKET_SIZE: usize> PNSStrategy<RT, BUCKET_SIZE>
where
    RT: RoutingTable<BUCKET_SIZE>,
    for<'a> &'a RT: IntoIterator<Item = &'a Contact>,
{
    /// Update an existing contact in the table instead of inserting.
    fn update_existing(&self, contact: Contact, table: &mut RT) -> InsertionStrategyResult {
        let existing = table.contact_mut(contact.id());
        assert!(
            existing.is_some(),
            "update_existing should only be called after detecting a id to be present"
        );
        let existing = existing.unwrap();
        assert_eq!(
            existing.id(),
            contact.id(),
            "Returned contact has to have same id"
        );

        // drop if received data is older than stored data
        if contact.state_seq_nr() < existing.state_seq_nr() {
            return InsertionStrategyResult::Dropped;
        }
        // Drop if path is longer
        if contact.path().len() > existing.path().len() {
            return InsertionStrategyResult::Dropped;
        }

        // Replace if seq_nr is greater (newer)
        if contact.state_seq_nr() > existing.state_seq_nr() {
            *existing = contact;
            return InsertionStrategyResult::Updated;
        }

        // Otherwise the seq_nr is equal

        // Drop if seq_nr is equal and contact is valid
        if existing.state() != &State::Valid {
            return InsertionStrategyResult::Dropped;
        }
        // Drop if same seq_nr but Age is older
        if contact.age() < existing.age() {
            return InsertionStrategyResult::Dropped;
        }
        // Drop if state is not valid and the new info doesn't avoid
        // all failed links
        if let State::Rediscovering(rds) = existing.state() {
            for link in &rds.failed_link_list {
                if contact.path().contains_link(link) {
                    return InsertionStrategyResult::Dropped;
                }
            }
        }

        // Finally: contact is newer, better or fixes a contact

        *existing = contact;

        InsertionStrategyResult::Updated
    }

    /// Check if the contact can replace an entry in the bucket it belongs to.
    fn replace_in_full_bucket(&self, contact: Contact, table: &mut RT) -> InsertionStrategyResult {
        let bucket = table.bucket_mut(contact.id());
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
                if acc.is_none() || acc.as_ref().unwrap().path().len() < contact.path().len() {
                    Some(contact)
                } else {
                    acc
                }
            });

        if let Some(replaceable) = replaceable {
            let old_id = replaceable.id().clone();
            *replaceable = contact;
            return InsertionStrategyResult::Replaced(old_id);
        }

        InsertionStrategyResult::Dropped
    }
}

impl<RT, NT, const BUCKET_SIZE: usize> InsertionStrategy<RT, NT, BUCKET_SIZE>
    for PNSStrategy<RT, BUCKET_SIZE>
where
    RT: RoutingTable<BUCKET_SIZE>,
    for<'a> &'a RT: IntoIterator<Item = &'a Contact>,
    NT: NeighborTable,
    for<'b> &'b NT: IntoIterator<Item = (&'b NodeId, &'b Port)>,
{
    fn insert(
        &mut self,
        mut contact: Contact,
        routing_table: &mut RT,
        neighbor_table: &NT,
    ) -> InsertionStrategyResult {
        // Ignore paths via us or contacts containing our own id
        if contact.path().contains(routing_table.root()) || contact.id() == routing_table.root() {
            return InsertionStrategyResult::Dropped;
        }
        // If the first element is no neighbor or the Path is empty -> Drop
        match contact.path().first().map(|id| neighbor_table.contains(id)) {
            None | Some(false) => return InsertionStrategyResult::Dropped,
            Some(true) => {}
        };

        // remove cycles and simplify
        // Uses the Path containing the id of the contact
        // itself to include it in the process
        let mut whole_path = contact.whole_path();
        whole_path.remove_cycles();
        PathSimplifier::from(&mut whole_path).simplify(routing_table, neighbor_table);
        whole_path.pop();
        *contact.path_mut() = whole_path;

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
            Ok(()) => InsertionStrategyResult::Inserted,
        }
    }
}
