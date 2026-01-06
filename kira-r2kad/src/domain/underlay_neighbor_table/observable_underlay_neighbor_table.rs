use derive_more::Display;

use crate::domain::NodeId;
use crate::domain::SafeStateSeqNr;
use crate::domain::UnderlayNeighborId;

use super::ULNTable;

use std::collections::HashMap;
use std::fmt::Debug;
use std::ops::Deref;

#[derive(Debug, Display, Clone)]
pub enum ULNTableEvent {
    /// The [StateSeqNr](crate::domain::SafeStateSeqNr) changed.
    #[display("The StateSeqNr of the node changed")]
    SSNChanged,
}

pub trait ULNTableObserver: Send {
    fn notify(&self, event: ULNTableEvent);
}

impl<F> ULNTableObserver for F
where
    F: Fn(ULNTableEvent),
    F: Send,
{
    fn notify(&self, event: ULNTableEvent) {
        self(event);
    }
}

pub struct ObservableULNTable<UN> {
    observers: Vec<Box<dyn ULNTableObserver>>,
    inner: UN,
}

impl<UN> ObservableULNTable<UN> {
    pub fn new(ulntable: UN) -> Self {
        Self {
            inner: ulntable,
            observers: Vec::default(),
        }
    }

    pub fn add_observer(&mut self, observer: impl ULNTableObserver + 'static) {
        self.observers.push(Box::new(observer));
    }

    fn emit(&self, event: ULNTableEvent) {
        for observer in &self.observers {
            observer.notify(event.clone());
        }
    }
}

impl<UN: Debug> Debug for ObservableULNTable<UN> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObservableULNTable")
            .field("observers", &self.observers.len())
            .field("inner", &self.inner)
            .finish()
    }
}

impl<UN: Default> Default for ObservableULNTable<UN> {
    fn default() -> Self {
        Self::new(Default::default())
    }
}

impl<UN> Deref for ObservableULNTable<UN>
where
    UN: Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    type Target = HashMap<NodeId, UnderlayNeighborId>;

    fn deref(&self) -> &Self::Target {
        self.inner.deref()
    }
}

impl<UN: ULNTable> ULNTable for ObservableULNTable<UN> {
    fn insert(&mut self, id: NodeId, ulnid: UnderlayNeighborId) -> Option<UnderlayNeighborId> {
        let last_ssn = *self.state_seq_nr();
        let last_mapping = self.inner.insert(id, ulnid);

        // because None can either mean fresh or no insert
        if self.state_seq_nr() != &last_ssn {
            self.emit(ULNTableEvent::SSNChanged);
        }

        last_mapping
    }

    fn contains(&self, id: &NodeId) -> bool {
        self.inner.contains(id)
    }

    fn state_seq_nr(&self) -> &SafeStateSeqNr {
        self.inner.state_seq_nr()
    }

    fn remove(&mut self, id: &NodeId) -> Option<UnderlayNeighborId> {
        let removed = self.inner.remove(id);

        if removed.is_some() {
            self.emit(ULNTableEvent::SSNChanged);
        }

        removed
    }

    fn size(&self) -> usize {
        self.inner.size()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use crate::domain::{ConnectionId, InMemoryULNTable, InterfaceId};

    use super::*;

    fn init_observer() -> (Arc<Mutex<bool>>, ObservableULNTable<InMemoryULNTable>) {
        let emitted = Arc::new(Mutex::new(false));
        let mut table: ObservableULNTable<InMemoryULNTable> = Default::default();
        {
            let emitted = emitted.clone();
            table.add_observer(move |_| {
                let mut lock = emitted.lock().unwrap();
                *lock = true;
            });
        }

        (emitted, table)
    }

    #[test]
    fn emit_on_insert() {
        let (emitted, mut table) = init_observer();

        let ulnid = {
            let interface_id = InterfaceId::try_from(42).unwrap();
            let conn_id = ConnectionId::from(42);

            UnderlayNeighborId {
                interface_id,
                connection_id: conn_id,
            }
        };

        let neighbor = NodeId::random();
        table.insert(neighbor, ulnid);
        assert!(*emitted.lock().unwrap(), "should notify after new neighbor");
        *emitted.lock().unwrap() = false;

        table.insert(neighbor, ulnid);
        assert!(
            !*emitted.lock().unwrap(),
            "no notification if same neighbor inserted"
        );

        let changed_ulnid = {
            let interface_id = InterfaceId::try_from(69).unwrap();
            let conn_id = ConnectionId::from(69);

            UnderlayNeighborId {
                interface_id,
                connection_id: conn_id,
            }
        };

        table.insert(neighbor, changed_ulnid);
        assert!(
            *emitted.lock().unwrap(),
            "should notify about neighbor update"
        );
        *emitted.lock().unwrap() = false;
    }

    #[test]
    fn no_emit() {
        let (emitted, table) = init_observer();

        table.contains(&NodeId::random());
        assert!(!*emitted.lock().unwrap(), "should not notify on contains");

        table.state_seq_nr();
        assert!(!*emitted.lock().unwrap(), "should not notify on ssn probe");
    }

    #[test]
    fn emit_remove() {
        let (emitted, mut table) = init_observer();
        let ulnid = {
            let interface_id = InterfaceId::try_from(42).unwrap();
            let conn_id = ConnectionId::from(42);

            UnderlayNeighborId {
                interface_id,
                connection_id: conn_id,
            }
        };

        table.remove(&NodeId::random());
        assert!(
            !*emitted.lock().unwrap(),
            "no notification if neighbor wasn't present"
        );

        let neighbor = NodeId::random();
        table.insert(neighbor, ulnid);
        assert!(*emitted.lock().unwrap(), "should notify about changed SSN");
        *emitted.lock().unwrap() = false;

        table.remove(&neighbor);
        assert!(
            *emitted.lock().unwrap(),
            "should notify if neighbor removed"
        );
    }

    #[test]
    fn upholds_trait_invariants() {
        use super::super::tests;

        let (_, table) = init_observer();
        tests::ssn_on_insert(table);

        let (_, table) = init_observer();
        tests::ssn_on_remove(table);

        let (_, table) = init_observer();
        tests::ssn_on_fake_remove(table);
    }
}
