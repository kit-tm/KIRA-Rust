use std::marker::PhantomData;

use crate::domain::{AddError, Contact, InsertionError, NodeId, RoutingTable};

#[derive(Debug, Eq, PartialEq)]
pub enum InsertionStrategyResult<const ID_SIZE: usize> {
    Dropped,
    Inserted,
    Replaced(NodeId<ID_SIZE>),
    Updated,
}

/// An [InsertionResult] which handles inserting a [Contact] into a [RoutingTable].
///
/// The actions performed with the [Contact] are limited to the [InsertionStrategyResult].
///
/// Insertion asserts the [Contact] to be valid.
/// That means the [Path] is not via failed [Link]s, the Path is shortened, the physical neighbor
/// the Path is mentioning (first entry) is known and valid.
pub trait InsertionStrategy<
    'a,
    RT: RoutingTable<'a, ID_SIZE, BUCKET_SIZE>,
    const ID_SIZE: usize,
    const BUCKET_SIZE: usize,
>
{
    /// Insert the [Contact] into the [RoutingTable].
    ///
    /// Will return the action performed for the [Contact].
    fn insert(
        &mut self,
        contact: Contact<ID_SIZE>,
        table: &mut RT,
    ) -> InsertionStrategyResult<ID_SIZE>;
}

/// This implementation inserts physical neighbors into its buckets replacing non-neighbor
/// contacts until the whole bucket is filled with physical neighbors.
/// After this point physical neighbors are only replaced if they
pub struct PNSPRStrategy<RT, const ID_SIZE: usize, const BUCKET_SIZE: usize> {
    _pd: PhantomData<RT>,
}

impl<
        'a,
        RT: RoutingTable<'a, ID_SIZE, BUCKET_SIZE>,
        const ID_SIZE: usize,
        const BUCKET_SIZE: usize,
    > PNSPRStrategy<RT, ID_SIZE, BUCKET_SIZE>
{
    /// Update an existing contact in the table instead of inserting.
    fn update_existing(
        &self,
        contact: Contact<ID_SIZE>,
        table: &mut RT,
    ) -> InsertionStrategyResult<ID_SIZE> {
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

        // received data is older than stored data
        if contact.age() > existing.age() || contact.state_seq_nr() > existing.state_seq_nr() {
            return InsertionStrategyResult::Dropped;
        }
        *existing = contact;

        InsertionStrategyResult::Updated
    }

    /// Check if the contact can replace an entry in the bucket if belongs to.
    fn replace_in_full_bucket(
        &self,
        contact: Contact<ID_SIZE>,
        table: &mut RT,
    ) -> InsertionStrategyResult<ID_SIZE> {
        let bucket = table.bucket_mut(contact.id());

        // Get the contact with the longest path
        let replaceable =
            bucket
                .iter_mut()
                .fold(Option::<&mut Contact<ID_SIZE>>::None, |acc, contact| {
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

impl<
        'a,
        RT: RoutingTable<'a, ID_SIZE, BUCKET_SIZE>,
        const ID_SIZE: usize,
        const BUCKET_SIZE: usize,
    > InsertionStrategy<'a, RT, ID_SIZE, BUCKET_SIZE> for PNSPRStrategy<RT, ID_SIZE, BUCKET_SIZE>
{
    fn insert(
        &mut self,
        contact: Contact<ID_SIZE>,
        table: &mut RT,
    ) -> InsertionStrategyResult<ID_SIZE> {
        // Ignore paths via us
        assert!(
            !contact.path().contains(table.root()),
            "Poisonous paths should be dropped before calling the InsertionStrategy"
        );
        // TODO: Check the neighbor in the Path to be valid (KadRoutingTable.cc, Zeile 215 - 353)

        match table.insert(contact.clone()) {
            Err(InsertionError::BucketSplit(_)) => self.replace_in_full_bucket(contact, table),
            Err(InsertionError::Add(AddError::AlreadyExists(_))) => {
                self.update_existing(contact, table)
            }
            Err(InsertionError::Add(AddError::NotAdded)) => {
                self.replace_in_full_bucket(contact, table)
            }
            Ok(()) => InsertionStrategyResult::Inserted,
        }
    }
}
