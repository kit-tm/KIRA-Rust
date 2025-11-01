use std::fmt::{Debug, Display, Formatter};
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use crate::domain::unlimited_uln_routing_table::UnlimitedULNRoutingTable;
use crate::domain::{
    AddError, Bucket, BucketSplitError, Contact, FlatRoutingTable, GroupingError, NodeId,
    ReplacementError, RoutingTable, SharedPrefix,
};

/// An Event emitted by the [ObservableRoutingTable].
///
/// For performance reasons the Updated*-Events only include the new value.
/// Otherwise the previous value has to be cloned as well.
///
/// # Types of events
///
/// The _contact* events_ are all actually published by the [ObservableRoutingTable]
/// when any new contact is added, inserted, updated or removed.
///
/// In contrast to them the _bucket* events_ can't be tracked that easily.
/// If a table creates a new bucket or updates an existing one is up to the types
/// implementing [RoutingTable] and wrapped by [ObservableRoutingTable].
/// Therefore [RoutingTableEvent::UpdatedBucket] is currently (2022.07.13) the only one
/// triggered from the [ObservableRoutingTable].
///
/// # Explicitly emitting an event
///
/// To publish an event from explicitly one can use the [ObservableRoutingTable::emit] function.
/// This can be handy when e.g. an
/// [InsertionStrategy](crate::domain::insertion_strategy::InsertionStrategy) explicitly splits a
/// bucket and filters out some contacts.
/// The [InsertionStrategy](crate::domain::insertion_strategy::InsertionStrategy) can then emit
/// multiple [RoutingTableEvent::RemovedContact] explicitly while otherwise the
/// [ObservableRoutingTable] would only emit one [RoutingTableEvent::UpdatedBucket].
#[derive(Debug, Eq, PartialEq, Clone)]
pub enum RoutingTableEvent<const BUCKET_SIZE: usize> {
    /// The new [Contact] was added to the [RoutingTable].
    NewContact(Contact),
    /// An existing [Contact] was updated.
    UpdatedContact { new: Contact, old: Contact },
    /// A [Contact] was removed from the [RoutingTable].
    RemovedContact(Contact),
    /// The [Bucket] at the given index was updated.
    /// This event is only fire when a path of a [Contact] in the [Bucket] changed.
    UpdatedBucket(usize),
    /// A new [Bucket] was added at the given index.
    NewBucket(usize),
}

impl<const BUCKET_SIZE: usize> Display for RoutingTableEvent<BUCKET_SIZE> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NewContact(contact) => write!(f, "NewContact [{contact}]"),
            Self::RemovedContact(contact) => write!(f, "RemovedContact [{contact}]"),
            Self::UpdatedContact { old, new } => write!(f, "UpdatedContact [{old} => {new}]"),
            Self::NewBucket(bucket) => write!(f, "NewBucket [{bucket}]"),
            Self::UpdatedBucket(bucket) => write!(f, "UpdatedBucket to [{bucket}]"),
        }
    }
}

/// Observer for [RoutingTableEvent]s.
///
/// The trait is implemented for all types implementing `Fn(RoutingTableEvent<BUCKET_SIZE>)`
/// which includes static functions and closures.
/// For more complex types its recommended to implement [RoutingTableObserver] directly.
///
/// # Example
///
/// A simple way to implement A [RoutingTableEvent] is by using a closure.
///
/// ```rust
/// # use kira_r2kad::domain::{Bucket, Contact, FlatRoutingTable, Path, RoutingTable,
/// # SafeStateSeqNr, NodeId};
/// # use kira_r2kad::domain::observable_routing_table::{ObservableRoutingTable, RoutingTableEvent};
/// let mut observable_rt = ObservableRoutingTable::from(FlatRoutingTable::default(NodeId::zero()));
///
/// // A simple debug logger
/// observable_rt.add_observer(|event| println!("{:?}", event));
///
/// let contact = Contact::new(
///     Path::from(NodeId::with_msb(2)),
///     SafeStateSeqNr::try_from(2).unwrap(),
/// );
/// let _ = observable_rt.add(contact);
///
/// ```
pub trait RoutingTableObserver<const BUCKET_SIZE: usize>: Send {
    /// Notify the [RoutingTableObserver] about a [RoutingTableEvent].
    fn notify(&self, event: RoutingTableEvent<BUCKET_SIZE>);
}

impl<F, const BUCKET_SIZE: usize> RoutingTableObserver<BUCKET_SIZE> for F
where
    F: Fn(RoutingTableEvent<BUCKET_SIZE>),
    F: Send,
{
    fn notify(&self, event: RoutingTableEvent<BUCKET_SIZE>) {
        self(event);
    }
}

/// Helper-Function to notify a slice of observers.
fn notify_all<const BUCKET_SIZE: usize>(
    observers: &[Box<dyn RoutingTableObserver<BUCKET_SIZE>>],
    event: RoutingTableEvent<BUCKET_SIZE>,
) {
    // Skip update events that only update the age
    if let RoutingTableEvent::UpdatedContact { old, new } = &event {
        if old.path() == new.path()
            && old.state_seq_nr() == new.state_seq_nr()
            && old.state() == new.state()
        {
            return;
        }
    }

    for observable in observers {
        observable.notify(event.clone());
    }
}

/// An type which implements [RoutingTable] but doesn't support observability.
///
/// Marker trait for implementations of [RoutingTable] which don't implement
/// an observable pattern and can be wrapped by [ObservableRoutingTable].
pub trait NonObservableRoutingTable<'a, const BUCKET_SIZE: usize>:
    RoutingTable<'a, BUCKET_SIZE>
{
}

impl<const BUCKET_SIZE: usize, const ACC: usize> NonObservableRoutingTable<'_, BUCKET_SIZE>
    for FlatRoutingTable<BUCKET_SIZE, ACC>
{
}

impl<const BUCKET_SIZE: usize, const ACC: usize> NonObservableRoutingTable<'_, BUCKET_SIZE>
    for UnlimitedULNRoutingTable<BUCKET_SIZE, ACC>
{
}

/// Wrapper for [NonObservableRoutingTable]s to gain observability.
pub struct ObservableRoutingTable<RT, const BUCKET_SIZE: usize> {
    observers: Vec<Box<dyn RoutingTableObserver<BUCKET_SIZE>>>,
    inner: RT,
}

impl<RT: Debug, const BUCKET_SIZE: usize> Debug for ObservableRoutingTable<RT, BUCKET_SIZE> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObservableRoutingTable")
            .field("observables", &self.observers.len())
            .field("inner", &self.inner)
            .finish()
    }
}

impl<RT, const BUCKET_SIZE: usize> From<RT> for ObservableRoutingTable<RT, BUCKET_SIZE>
where
    for<'a> RT: RoutingTable<'a, BUCKET_SIZE>,
{
    fn from(routing_table: RT) -> Self {
        Self {
            observers: Vec::new(),
            inner: routing_table,
        }
    }
}

impl<RT, const BUCKET_SIZE: usize> ObservableRoutingTable<RT, BUCKET_SIZE> {
    /// Adds any type of [RoutingTableObserver] to the list of observers.
    pub fn add_observer<O: 'static + RoutingTableObserver<BUCKET_SIZE>>(&mut self, observer: O) {
        self.observers.push(Box::new(observer));
    }

    /// Explicitly emits a [RoutingTableEvent] to all observers of this [ObservableRoutingTable].
    ///
    /// This can be used to emit additional events to the ones automatically emitted.
    /// E.g. when using [RoutingTable::bucket_mut] instead of [RoutingTable::remove]
    /// to remove multiple contacts, [ObservableRoutingTable::emit] can be used to
    /// explicitly emit [RoutingTableEvent::RemovedContact]-Events for every contact
    /// removed.
    pub fn emit(&self, event: RoutingTableEvent<BUCKET_SIZE>) {
        self.notify_all(event);
    }

    // Eases the access to the utility function
    fn notify_all(&self, event: RoutingTableEvent<BUCKET_SIZE>) {
        notify_all(&self.observers, event);
    }
}

impl<'a, RT, const BUCKET_SIZE: usize> RoutingTable<'a, BUCKET_SIZE>
    for ObservableRoutingTable<RT, BUCKET_SIZE>
where
    RT: 'a + NonObservableRoutingTable<'a, BUCKET_SIZE>,
{
    type ContactWriteGuard = ContactWriteGuard<'a, RT::ContactWriteGuard, BUCKET_SIZE>;
    type BucketIter = RT::BucketIter;

    fn root(&self) -> &NodeId {
        self.inner.root()
    }

    fn len(&self) -> usize {
        self.inner.len()
    }

    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    fn add(&mut self, contact: Contact) -> Result<(), AddError> {
        self.inner.add(contact.clone())?;
        self.notify_all(RoutingTableEvent::NewContact(contact));
        Ok(())
    }

    fn remove(&mut self, id: &NodeId) -> Option<Contact> {
        let contact = self.inner.remove(id)?;
        self.notify_all(RoutingTableEvent::RemovedContact(contact.clone()));
        Some(contact)
    }

    fn replace(&mut self, id: &NodeId, with: Contact) -> Result<Contact, ReplacementError> {
        let replaced = self.inner.replace(id, with.clone())?;
        self.notify_all(RoutingTableEvent::UpdatedContact {
            new: with,
            old: replaced.clone(),
        });
        Ok(replaced)
    }

    fn contact(&self, id: &NodeId) -> Option<&Contact> {
        self.inner.contact(id)
    }

    fn random_id(&self) -> Option<&NodeId> {
        self.inner.random_id()
    }

    fn contact_mut(&'a mut self, id: &NodeId) -> Option<Self::ContactWriteGuard> {
        self.inner.contact_mut(id).map(|contact| ContactWriteGuard {
            observers: &mut self.observers,
            original: Contact::clone(contact.deref()),
            contact,
        })
    }

    fn contains(&self, id: &NodeId) -> bool {
        self.inner.contains(id)
    }

    fn split_bucket(&mut self, id: &NodeId) -> Result<usize, BucketSplitError> {
        let bucket_old = self.inner.bucket(id).clone();
        let index = self.inner.split_bucket(id)?;

        if bucket_old
            .iter()
            .zip(self.inner.bucket_by_index(index))
            .any(|(a, b)| a.path() != b.path())
        {
            self.notify_all(RoutingTableEvent::UpdatedBucket(index));
        }

        self.notify_all(RoutingTableEvent::NewBucket(index + 1));
        Ok(index)
    }

    fn bucket(&self, of: &NodeId) -> &Bucket<BUCKET_SIZE> {
        self.inner.bucket(of)
    }

    fn closest(
        &self,
        to: &NodeId,
        n: usize,
        shared_prefix_grouping: usize,
    ) -> Result<Vec<(SharedPrefix, Contact)>, GroupingError> {
        self.inner.closest(to, n, shared_prefix_grouping)
    }

    fn iter(&self) -> impl Iterator<Item = &Contact> {
        self.inner.iter()
    }

    fn iter_mut(&'a mut self) -> impl Iterator<Item = Self::ContactWriteGuard> {
        Iter::new(&self.observers, self.inner.iter_mut())
    }

    fn bucket_iter(&'a self) -> Self::BucketIter {
        self.inner.bucket_iter()
    }

    fn bucket_by_index(&self, index: usize) -> &Bucket<BUCKET_SIZE> {
        self.inner.bucket_by_index(index)
    }

    fn get_bucket_index(&self, of: &NodeId) -> usize {
        self.inner.get_bucket_index(of)
    }

    fn get_bucket_prefix_length(&self, bucket_index: usize) -> usize {
        self.inner.get_bucket_prefix_length(bucket_index)
    }
}

// Not using "NonObservableRoutingTable" Trait as rust emits recursion error (maybe a rust bug?)

pub struct Iter<'a, I, C, const BUCKET_SIZE: usize> {
    observers: &'a [Box<dyn RoutingTableObserver<BUCKET_SIZE>>],
    inner_iter: I,
    // To keep 'a and C
    _pd: PhantomData<&'a C>,
}

impl<'a, I, C, const BUCKET_SIZE: usize> Iter<'a, I, C, BUCKET_SIZE> {
    fn new(observers: &'a [Box<dyn RoutingTableObserver<BUCKET_SIZE>>], inner_iter: I) -> Self {
        Self {
            observers,
            inner_iter,
            _pd: PhantomData,
        }
    }
}

impl<'a, I, C, const BUCKET_SIZE: usize> Iterator for Iter<'a, I, C, BUCKET_SIZE>
where
    I: Iterator<Item = C>,
    C: DerefMut<Target = Contact>,
{
    type Item = ContactWriteGuard<'a, C, BUCKET_SIZE>;

    fn next(&mut self) -> Option<Self::Item> {
        let next = self.inner_iter.next();

        next.map(|contact| ContactWriteGuard {
            observers: self.observers,
            original: Contact::clone(&contact),
            contact,
        })
    }
}

/// Resource Acquisition Is Initialization (RAII) type to provide observability of mutating a
/// [Contact].
///
/// Emits a [RoutingTableEvent::UpdatedContact] on [Drop] only if the [Contact] actually changed.
/// As a smart pointer this type dereferences to the updated [Contact].
pub struct ContactWriteGuard<'a, C, const BUCKET_SIZE: usize>
where
    C: DerefMut<Target = Contact>,
{
    observers: &'a [Box<dyn RoutingTableObserver<BUCKET_SIZE>>],
    original: Contact,
    contact: C,
}

impl<C, const BUCKET_SIZE: usize> Deref for ContactWriteGuard<'_, C, BUCKET_SIZE>
where
    C: DerefMut<Target = Contact>,
{
    type Target = Contact;

    fn deref(&self) -> &Self::Target {
        self.contact.deref()
    }
}

impl<C, const BUCKET_SIZE: usize> DerefMut for ContactWriteGuard<'_, C, BUCKET_SIZE>
where
    C: DerefMut<Target = Contact>,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.contact.deref_mut()
    }
}

impl<C, const BUCKET_SIZE: usize> Drop for ContactWriteGuard<'_, C, BUCKET_SIZE>
where
    C: DerefMut<Target = Contact>,
{
    fn drop(&mut self) {
        if self.contact.deref() != &self.original {
            notify_all(
                self.observers,
                RoutingTableEvent::UpdatedContact {
                    new: self.contact.clone(),
                    old: self.original.clone(),
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, RwLock};

    use crate::domain::observable_routing_table::{ObservableRoutingTable, RoutingTableEvent};
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::domain::{
        Contact, ContactState, NodeId, Path, RoutingTable, SafeStateSeqNr, Timestamp,
    };

    #[test]
    fn add_contact() {
        let contact = Contact::new(
            Path::from(NodeId::with_msb(2)),
            SafeStateSeqNr::try_from(2).unwrap(),
        );

        let mut observable = ObservableRoutingTable::from(SingleBucketRT::<10>::new(NodeId::one()));

        let events = Arc::new(RwLock::new(vec![]));

        let observable_events = Arc::clone(&events);
        observable.add_observer(move |event| observable_events.write().unwrap().push(event));

        let add_result = observable.add(contact.clone());
        assert!(
            add_result.is_ok(),
            "Adding should be ok in empty routing table but was: {add_result:?}"
        );
        assert!((events.read().unwrap()).contains(&RoutingTableEvent::NewContact(contact)));
    }

    #[test]
    fn remove_contact() {
        let contact = Contact::new(
            Path::from(NodeId::with_msb(2)),
            SafeStateSeqNr::try_from(2).unwrap(),
        );

        let mut rt = SingleBucketRT::<10>::new(NodeId::one());
        assert!(rt.add(contact.clone()).is_ok());

        let mut observable = ObservableRoutingTable::from(rt);

        let events = Arc::new(RwLock::new(vec![]));

        let observable_events = Arc::clone(&events);
        observable.add_observer(move |event| observable_events.write().unwrap().push(event));

        let add_result = observable.remove(contact.id());
        assert!(add_result.is_some());
        assert!((events.read().unwrap()).contains(&RoutingTableEvent::RemovedContact(contact)));
    }

    #[test]
    fn replace_contact() {
        let contact = Contact::new(
            Path::from(NodeId::with_msb(2)),
            SafeStateSeqNr::try_from(2).unwrap(),
        );

        let mut rt = SingleBucketRT::<10>::new(NodeId::one());
        assert!(rt.add(contact.clone()).is_ok());

        let mut observable = ObservableRoutingTable::from(rt);

        let events = Arc::new(RwLock::new(vec![]));

        let observable_events = Arc::clone(&events);
        observable.add_observer(move |event| observable_events.write().unwrap().push(event));

        let new_contact = Contact::new(
            Path::from(NodeId::with_msb(3)),
            SafeStateSeqNr::try_from(3).unwrap(),
        );

        let add_result = observable.replace(contact.id(), new_contact.clone());
        assert!(add_result.is_ok());
        assert!(
            (events.read().unwrap()).contains(&RoutingTableEvent::UpdatedContact {
                old: contact,
                new: new_contact
            })
        );
    }

    #[test]
    fn update_contact() {
        let contact = Contact::new(
            Path::from(NodeId::with_msb(2)),
            SafeStateSeqNr::try_from(2).unwrap(),
        );

        let mut rt = SingleBucketRT::<10>::new(NodeId::one());
        assert!(rt.add(contact.clone()).is_ok());

        let mut observable = ObservableRoutingTable::from(rt);

        let events = Arc::new(RwLock::new(vec![]));

        let observable_events = Arc::clone(&events);
        observable.add_observer(move |event| observable_events.write().unwrap().push(event));

        let timestamp = Timestamp::now();
        {
            // Has to be in brackets to force dropping before testing
            let contact = observable.contact_mut(contact.id());
            assert!(contact.is_some());
            let mut contact = contact.unwrap();
            *contact.state_mut() = ContactState::Invalid;
            *contact.last_seen_mut() = timestamp;
        }

        let mut updated_contact = contact.clone();
        *updated_contact.state_mut() = ContactState::Invalid;
        *updated_contact.last_seen_mut() = timestamp;

        assert!(
            events
                .read()
                .unwrap()
                .contains(&RoutingTableEvent::UpdatedContact {
                    new: updated_contact,
                    old: contact,
                }),
            "{events:?}"
        );
    }
}
