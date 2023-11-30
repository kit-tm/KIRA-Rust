use std::cmp::Ordering;
use std::fmt::Debug;
use std::marker::PhantomData;

use crate::domain::{
    AddError, Contact, ContactState, InsertionError, NodeId, PathCycleRemover, RoutingTable,
};

use super::{PNTable, PathSimplifier};

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
/// The Algorithm can use the [PNTable] but is not allowed to insert into it.
/// This will be handled where the Hello-Messages are handled explicitly.
///
/// Also [NotVia](crate::domain::NotVia) Data is not handled by the [InsertionStrategy] as it
/// represents logic of the routing-daemon itself and not the domain.
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
    fn update_existing(&self, mut contact: Contact, table: &mut RT) -> InsertionStrategyResult {
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
            log::trace!(target: "routing_table", "Dropped path: Invalid [{:?}]", contact);
            return InsertionStrategyResult::Dropped;
        }
        if existing.state() == &ContactState::Invalid && contact.state() == &ContactState::Valid {
            log::trace!(target: "routing_table", "Updated path: Invalid path was replaced [{:?}]", contact);
            *existing = contact;
            return InsertionStrategyResult::Updated;
        }

        // drop if received data is older than stored data
        match existing.cmp_actuality(&contact) {
            Ordering::Greater => {
                log::trace!(
                    target: "routing_table",
                    "Dropping path: Older [info: {:?}, saved_age: {:?}, saved_ssn: {:?}]",
                    contact,
                    existing.age(),
                    existing.state_seq_nr(),
                );
                return InsertionStrategyResult::Dropped;
            }
            // If less or equal -> replace
            Ordering::Less => {}
            Ordering::Equal => {}
        }

        // contact is newer, better or fixes a contact

        // But: If only age is updated, don't emit anything
        let return_result =
            if contact.path() == existing.path() && contact.state() == existing.state()
                && contact.neighbor_sum() == existing.neighbor_sum() && contact.number_of_pn() == existing.number_of_pn() {
                log::trace!(
                    target: "routing_table",
                    "Not updating contacts path because its the same and doesn't change state [{}]",
                    contact.id()
                );
                InsertionStrategyResult::Dropped
            } else {
                log::trace!(
                    target: "routing_table",
                    "Updated contact [{:?}]",
                    contact
                );
                if (contact.number_of_pn().is_none() && existing.number_of_pn().is_some())
                    || (contact.neighbor_sum().is_none() && existing.neighbor_sum().is_some()) {
                    log::debug!("Warning: Removing neighbor sum due to contact update: {}, {:?}", contact, existing.number_of_pn())
                }
                InsertionStrategyResult::Updated
            };

        contact.set_last_seen_now();
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

        // Get the contact with the longest path but only if longer than new contact
        let replaceable = bucket
            .iter_mut()
            .filter(|c| c.path().size() > contact.path().size())
            // Invariant: acc contains the contact with the longest path in the bucket after processing the first entry
            .max_by_key(|c| c.path().size());

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

#[cfg(test)]
mod tests {
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::domain::{
        Contact, ContactState, InOrderCycleRemover, InsertionStrategy, InsertionStrategyResult,
        NetworkInterface, NodeId, PNSStrategy, PNTable, Path, RoutingTable,
        ShortestFirstPathSimplifier, StateSeqNr,
    };

    #[test]
    fn extract_infos_from_find_node_not_for_us() {
        crate::tests::init();

        /* Topology:
                         /---- target
           neighbor -- root -x- other_neighbor -- proxy_invalidated
               \----------------------------------/
           (Where -x- signals a broken link)

           Test: root should update its contact for proxy_invalidated after receiving
                 a FindNodeRsp which is not directed to him.
        */

        let root_id = NodeId::with_msb(1);
        let neighbor_id = NodeId::with_msb(2);
        let other_neighbor_id = NodeId::with_msb(3);
        let target_id = NodeId::with_msb(4);
        let proxy_invalidated_id = NodeId::with_msb(5);

        let interface = NetworkInterface::new("test");
        let other_interface = NetworkInterface::new("test 2");
        let third_interface = NetworkInterface::new("test 3");

        let mut proxy_invalidated_contact = Contact::new(
            Path::from([other_neighbor_id.clone(), proxy_invalidated_id.clone()]),
            StateSeqNr::from(5),
        );
        *proxy_invalidated_contact.state_mut() = ContactState::Invalid;

        let mut other_neighbor_contact =
            Contact::new(Path::from([other_neighbor_id.clone()]), StateSeqNr::from(3));
        *other_neighbor_contact.state_mut() = ContactState::Invalid;

        let neighbor_contact = Contact::new(Path::from([neighbor_id.clone()]), StateSeqNr::from(2));

        let target_contact = Contact::new(Path::from([target_id.clone()]), StateSeqNr::from(4));

        let mut single_bucket_rt = SingleBucketRT::<20>::new(root_id.clone());

        for contact in [
            proxy_invalidated_contact,
            other_neighbor_contact,
            neighbor_contact,
            target_contact,
        ] {
            assert!(
                single_bucket_rt.insert(contact.clone()).is_ok(),
                "Failed to insert {:?}",
                contact
            );
        }

        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());
        pn_table.insert(other_neighbor_id.clone(), other_interface.clone());
        pn_table.insert(target_id.clone(), third_interface.clone());

        let mut insertion_strategy =
            PNSStrategy::new(InOrderCycleRemover, ShortestFirstPathSimplifier);

        // updated contact is inserted
        let updated_contact = Contact::new(
            Path::from([neighbor_id.clone(), proxy_invalidated_id.clone()]),
            StateSeqNr::from(5),
        );

        let result = insertion_strategy.insert(updated_contact, &mut single_bucket_rt, &pn_table);

        assert_eq!(
            result,
            InsertionStrategyResult::Updated,
            "Previously invalid contact should be updated"
        );

        // Should contain the previously invalidated contact
        let contact = single_bucket_rt.contact(&proxy_invalidated_id);
        assert!(contact.is_some(), "couldn't find invalidated contact");
        let contact = contact.unwrap();
        assert_eq!(
            contact.state(),
            &ContactState::Valid,
            "Contact should be validated"
        );
        assert_eq!(
            contact.path(),
            &Path::from([neighbor_id.clone(), proxy_invalidated_id.clone()]),
            "Path was unexpectedly changed"
        );
    }
}
