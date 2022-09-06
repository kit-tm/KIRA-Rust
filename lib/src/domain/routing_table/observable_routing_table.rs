use std::fmt::{Debug, Display, Formatter};
use std::ops::{Deref, DerefMut};

use crate::domain::unlimited_pn_routing_table::UnlimitedPNRoutingTable;
use crate::domain::{
    AddError, Bucket, BucketSplitError, Contact, FlatRoutingTable, NodeId, ReplacementError,
    RoutingTable,
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
/// To publish an event from explicitly one can use the [ObservableRutingTable::emit] function.
/// This can be handy when e.g. an [InsertionStrategy] explicitly splits a bucket and filters out
/// some contacts.
/// The [InsertionStrategy] can then emit multiple [RoutingTableEvent::RemovedContact] explicitly
/// while otherwise the [ObservableRoutingTable] would only emit one
/// [RoutingTableEvent::UpdatedBucket].
#[derive(Debug, Eq, PartialEq, Clone)]
pub enum RoutingTableEvent<const BUCKET_SIZE: usize> {
    /// The new [Contact] was added to the [RoutingTable].
    NewContact(Contact),
    /// An existing [Contact] was updated.
    UpdatedContact { new: Contact, old: Contact },
    /// A [Contact] was removed from the [RoutingTable].
    RemovedContact(Contact),
    /// A [Bucket] was updated to the given value.
    UpdatedBucket(Bucket<BUCKET_SIZE>),
    /// The given new [Bucket] was added to the [RoutingTable].
    NewBucket(Bucket<BUCKET_SIZE>),
}

impl<const BUCKET_SIZE: usize> Display for RoutingTableEvent<BUCKET_SIZE> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NewContact(contact) => write!(f, "NewContact [{}]", contact),
            Self::RemovedContact(contact) => write!(f, "RemovedContact [{}]", contact),
            Self::UpdatedContact { old, new } => write!(f, "UpdatedContact [{} => {}]", old, new),
            Self::NewBucket(bucket) => write!(f, "NewBucket [{}]", bucket),
            Self::UpdatedBucket(bucket) => write!(f, "UpdatedBucket to [{}]", bucket),
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
/// # use r2kad_lib::domain::{Bucket, FlatRoutingTable, NodeId};
/// # use r2kad_lib::domain::observable_routing_table::{ObservableRoutingTable, RoutingTableEvent};
/// let mut observable_rt = ObservableRoutingTable::from(FlatRoutingTable::default(NodeId::zero()));
///
/// // A simple debug logger
/// observable_rt.add_observer(|event| println!("{:?}", event));
///
/// observable_rt.emit(RoutingTableEvent::NewBucket(Bucket::new()));
/// ```
pub trait RoutingTableObserver<const BUCKET_SIZE: usize> {
    /// Notify the [RoutingTableObserver] about a [RoutingTableEvent].
    fn notify(&self, event: RoutingTableEvent<BUCKET_SIZE>);
}

impl<F, const BUCKET_SIZE: usize> RoutingTableObserver<BUCKET_SIZE> for F
where
    F: Fn(RoutingTableEvent<BUCKET_SIZE>),
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

impl<'a, const BUCKET_SIZE: usize, const ACC: usize> NonObservableRoutingTable<'a, BUCKET_SIZE>
    for FlatRoutingTable<BUCKET_SIZE, ACC>
{
}

impl<'a, const BUCKET_SIZE: usize, const ACC: usize> NonObservableRoutingTable<'a, BUCKET_SIZE>
    for UnlimitedPNRoutingTable<BUCKET_SIZE, ACC>
{
}

/// Wrapper for [NonObservableRoutingTable]s to gain observability.
pub struct ObservableRoutingTable<RT, const BUCKET_SIZE: usize> {
    observables: Vec<Box<dyn RoutingTableObserver<BUCKET_SIZE>>>,
    inner: RT,
}

impl<RT: Debug, const BUCKET_SIZE: usize> Debug for ObservableRoutingTable<RT, BUCKET_SIZE> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ObservableRoutingTable [observables: {}, inner: {:?}]",
            self.observables.len(),
            self.inner
        )
    }
}

impl<RT, const BUCKET_SIZE: usize> From<RT> for ObservableRoutingTable<RT, BUCKET_SIZE>
where
    for<'a> RT: RoutingTable<'a, BUCKET_SIZE>,
{
    fn from(routing_table: RT) -> Self {
        Self {
            observables: Vec::new(),
            inner: routing_table,
        }
    }
}

impl<RT, const BUCKET_SIZE: usize> ObservableRoutingTable<RT, BUCKET_SIZE> {
    /// Adds any type of [RoutingTableObserver] to the list of observers.
    pub fn add_observer<O: 'static + RoutingTableObserver<BUCKET_SIZE>>(&mut self, observer: O) {
        self.observables.push(Box::new(observer));
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
        notify_all(&self.observables, event);
    }
}

impl<'a, RT, const BUCKET_SIZE: usize> RoutingTable<'a, BUCKET_SIZE>
    for ObservableRoutingTable<RT, BUCKET_SIZE>
where
    RT: NonObservableRoutingTable<'a, BUCKET_SIZE>,
{
    type ContactWriteGuard = ContactWriteGuard<'a, RT::ContactWriteGuard, BUCKET_SIZE>;
    type BucketWriteGuard = BucketWriteGuard<'a, RT::BucketWriteGuard, BUCKET_SIZE>;

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
        let result = self.inner.add(contact.clone());
        if result.is_ok() {
            self.notify_all(RoutingTableEvent::NewContact(contact.clone()));

            let bucket_after = self.bucket(contact.id()).clone();
            self.notify_all(RoutingTableEvent::UpdatedBucket(bucket_after));
        }

        result
    }

    fn remove(&mut self, id: &NodeId) -> Option<Contact> {
        let id = id.clone();

        let result = self.inner.remove(&id);
        if let Some(contact) = result.as_ref().cloned() {
            self.notify_all(RoutingTableEvent::RemovedContact(contact));
        }
        result
    }

    fn replace(&mut self, id: &NodeId, with: Contact) -> Result<Contact, ReplacementError> {
        let replaced = self.inner.replace(id, with.clone());
        if let Ok(contact) = replaced.as_ref().cloned() {
            self.notify_all(RoutingTableEvent::RemovedContact(contact));
            self.notify_all(RoutingTableEvent::NewContact(with));
        }
        replaced
    }

    fn contact(&self, id: &NodeId) -> Option<&Contact> {
        self.inner.contact(id)
    }

    fn random_id(&self) -> Option<&NodeId> {
        self.inner.random_id()
    }

    fn contact_mut(&'a mut self, id: &NodeId) -> Option<Self::ContactWriteGuard> {
        self.inner.contact_mut(id).map(|contact| ContactWriteGuard {
            observers: &mut self.observables,
            original: Contact::clone(contact.deref()),
            contact,
        })
    }

    fn contains(&self, id: &NodeId) -> bool {
        self.inner.contains(id)
    }

    fn split_bucket(&mut self, id: &NodeId) -> Result<(), BucketSplitError> {
        self.inner.split_bucket(id)
    }

    fn bucket(&self, of: &NodeId) -> &Bucket<BUCKET_SIZE> {
        self.inner.bucket(of)
    }

    fn bucket_mut(&'a mut self, of: &NodeId) -> Self::BucketWriteGuard {
        let bucket = self.inner.bucket_mut(of);
        BucketWriteGuard {
            observers: &self.observables,
            original: Bucket::<BUCKET_SIZE>::clone(bucket.deref()),
            bucket,
        }
    }

    fn get_closest(&self, to: &NodeId, shared_prefix_grouping: usize) -> Option<&Contact> {
        self.inner.get_closest(to, shared_prefix_grouping)
    }
}

impl<'a, RT, const BUCKET_SIZE: usize> IntoIterator for &'a ObservableRoutingTable<RT, BUCKET_SIZE>
where
    &'a RT: IntoIterator<Item = &'a Contact>,
{
    type Item = &'a Contact;
    type IntoIter = <&'a RT as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.inner.into_iter()
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

impl<'a, C, const BUCKET_SIZE: usize> Deref for ContactWriteGuard<'a, C, BUCKET_SIZE>
where
    C: DerefMut<Target = Contact>,
{
    type Target = Contact;

    fn deref(&self) -> &Self::Target {
        self.contact.deref()
    }
}

impl<'a, C, const BUCKET_SIZE: usize> DerefMut for ContactWriteGuard<'a, C, BUCKET_SIZE>
where
    C: DerefMut<Target = Contact>,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.contact.deref_mut()
    }
}

impl<'a, C, const BUCKET_SIZE: usize> Drop for ContactWriteGuard<'a, C, BUCKET_SIZE>
where
    C: DerefMut<Target = Contact>,
{
    fn drop(&mut self) {
        if self.contact.deref() != &self.original {
            notify_all(
                self.observers,
                RoutingTableEvent::UpdatedContact {
                    new: self.contact.deref().clone(),
                    old: self.original.clone(),
                },
            )
        }
    }
}

/// Resource Acquisition Is Initialization (RAII) type to provide observability of mutating a
/// [Bucket].
///
/// Emits a [RoutingTableEvent::UpdatedBucket] on [Drop] only if the [Bucket] actually changed.
/// As a smart pointer this type dereferences to the updated [Bucket].
pub struct BucketWriteGuard<'a, B, const BUCKET_SIZE: usize>
where
    B: DerefMut<Target = Bucket<BUCKET_SIZE>>,
{
    observers: &'a [Box<dyn RoutingTableObserver<BUCKET_SIZE>>],
    original: Bucket<BUCKET_SIZE>,
    bucket: B,
}

impl<'a, B, const BUCKET_SIZE: usize> Deref for BucketWriteGuard<'a, B, BUCKET_SIZE>
where
    B: DerefMut<Target = Bucket<BUCKET_SIZE>>,
{
    type Target = Bucket<BUCKET_SIZE>;

    fn deref(&self) -> &Self::Target {
        self.bucket.deref()
    }
}

impl<'a, B, const BUCKET_SIZE: usize> DerefMut for BucketWriteGuard<'a, B, BUCKET_SIZE>
where
    B: DerefMut<Target = Bucket<BUCKET_SIZE>>,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.bucket.deref_mut()
    }
}

impl<'a, B, const BUCKET_SIZE: usize> Drop for BucketWriteGuard<'a, B, BUCKET_SIZE>
where
    B: DerefMut<Target = Bucket<BUCKET_SIZE>>,
{
    fn drop(&mut self) {
        if self.bucket.deref() != &self.original {
            notify_all(
                self.observers,
                RoutingTableEvent::UpdatedBucket(self.bucket.deref().clone()),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use crate::domain::observable_routing_table::{ObservableRoutingTable, RoutingTableEvent};
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::domain::{Contact, ContactState, NodeId, Path, RoutingTable, StateSeqNr, Timestamp};

    #[test]
    fn add_contact() {
        let contact = Contact::new(Path::from(NodeId::with_msb(2)), StateSeqNr::from(1));

        let mut observable = ObservableRoutingTable::from(SingleBucketRT::<10>::new(NodeId::one()));

        let events = Rc::new(RefCell::new(vec![]));

        let observable_events = Rc::clone(&events);
        observable.add_observer(move |event| observable_events.borrow_mut().push(event));

        let add_result = observable.add(contact.clone());
        assert!(
            add_result.is_ok(),
            "Adding should be ok in empty routing table but was: {:?}",
            add_result
        );
        assert!((events.borrow()).contains(&RoutingTableEvent::NewContact(contact)));
    }

    #[test]
    fn remove_contact() {
        let contact = Contact::new(Path::from(NodeId::with_msb(2)), StateSeqNr::from(1));

        let mut rt = SingleBucketRT::<10>::new(NodeId::one());
        assert!(rt.add(contact.clone()).is_ok());

        let mut observable = ObservableRoutingTable::from(rt);

        let events = Rc::new(RefCell::new(vec![]));

        let observable_events = Rc::clone(&events);
        observable.add_observer(move |event| observable_events.borrow_mut().push(event));

        let add_result = observable.remove(contact.id());
        assert!(add_result.is_some());
        assert!((events.borrow()).contains(&RoutingTableEvent::RemovedContact(contact)));
    }

    #[test]
    fn replace_contact() {
        let contact = Contact::new(Path::from(NodeId::with_msb(2)), StateSeqNr::from(1));

        let mut rt = SingleBucketRT::<10>::new(NodeId::one());
        assert!(rt.add(contact.clone()).is_ok());

        let mut observable = ObservableRoutingTable::from(rt);

        let events = Rc::new(RefCell::new(vec![]));

        let observable_events = Rc::clone(&events);
        observable.add_observer(move |event| observable_events.borrow_mut().push(event));

        let new_contact = Contact::new(Path::from(NodeId::with_msb(3)), StateSeqNr::from(2));

        let add_result = observable.replace(contact.id(), new_contact.clone());
        assert!(add_result.is_ok());
        assert!((events.borrow()).contains(&RoutingTableEvent::RemovedContact(contact)));
        assert!((events.borrow()).contains(&RoutingTableEvent::NewContact(new_contact)));
    }

    #[test]
    fn update_contact() {
        let contact = Contact::new(Path::from(NodeId::with_msb(2)), StateSeqNr::from(1));

        let mut rt = SingleBucketRT::<10>::new(NodeId::one());
        assert!(rt.add(contact.clone()).is_ok());

        let mut observable = ObservableRoutingTable::from(rt);

        let events = Rc::new(RefCell::new(vec![]));

        let observable_events = Rc::clone(&events);
        observable.add_observer(move |event| observable_events.borrow_mut().push(event));

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
                .borrow()
                .contains(&RoutingTableEvent::UpdatedContact {
                    new: updated_contact,
                    old: contact
                }),
            "{:?}",
            events
        );
    }
}
