use std::{fmt::Debug, time::Instant};

use derive_more::Display;

use super::VicinityGraph;
use crate::domain::{NodeId, Path, SafeStateSeqNr};

#[derive(Debug, Display, Clone)]
pub enum VicinityGraphEvent {
    /// A node in the vicinity was considered stale.
    #[display("Node removed from the vicinity: {_0}")]
    Removed(NodeId),
}

pub trait VicinityGraphObserver: Send {
    fn notify(&self, event: VicinityGraphEvent);
}

impl<F> VicinityGraphObserver for F
where
    F: Fn(VicinityGraphEvent),
    F: Send,
{
    fn notify(&self, event: VicinityGraphEvent) {
        self(event);
    }
}

pub struct ObservableVicinityGraph<V> {
    observers: Vec<Box<dyn VicinityGraphObserver>>,
    inner: V,
}

impl<V: Debug> Debug for ObservableVicinityGraph<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObservableVicinityGraph")
            .field("observers", &self.observers.len())
            .field("inner", &self.inner)
            .finish()
    }
}

impl<V> ObservableVicinityGraph<V> {
    pub fn new(vicinity_graph: V) -> Self {
        Self {
            inner: vicinity_graph,
            observers: Vec::default(),
        }
    }

    pub fn add_observer(&mut self, observer: impl VicinityGraphObserver + 'static) {
        self.observers.push(Box::new(observer));
    }

    fn emit(&self, event: VicinityGraphEvent) {
        for observer in &self.observers {
            observer.notify(event.clone());
        }
    }
}

impl<V: VicinityGraph> From<V> for ObservableVicinityGraph<V> {
    fn from(value: V) -> Self {
        Self::new(value)
    }
}

impl<V: VicinityGraph> VicinityGraph for ObservableVicinityGraph<V> {
    type Error = V::Error;

    fn insert(
        &mut self,
        node: NodeId,
        neighbors: impl IntoIterator<Item = NodeId>,
        ssn: SafeStateSeqNr,
        last_seen: Instant,
    ) -> Result<(), Self::Error> {
        self.inner.insert(node, neighbors, ssn, last_seen)?;

        // we don't generate VicinityGraph::Removed because we at most destroy links
        // to the neighbors not present anymore in `neighbors`

        Ok(())
    }

    fn remove(&mut self, node: &NodeId) -> bool {
        if self.inner.remove(node) {
            self.emit(VicinityGraphEvent::Removed(*node));
            true
        } else {
            false
        }
    }

    fn prune(&mut self, node: &NodeId) -> bool {
        if self.inner.prune(node) {
            self.emit(VicinityGraphEvent::Removed(*node));
            true
        } else {
            false
        }
    }

    fn remove_radius(&mut self) -> bool {
        // FIXME: generate VicinityGraphEvent::Removed
        self.inner.remove_radius()
    }

    fn nodes(&self) -> impl Iterator<Item = &NodeId> {
        self.inner.nodes()
    }

    fn paths(&self) -> impl Iterator<Item = Path> {
        self.inner.paths()
    }

    fn paths_to(&self, destination: NodeId) -> impl Iterator<Item = Path> {
        self.inner.paths_to(destination)
    }

    fn update_last_seen(&mut self, node: &NodeId, now: Instant) -> bool {
        self.inner.update_last_seen(node, now)
    }

    fn last_seen(&self, node: &NodeId) -> Option<Instant> {
        self.inner.last_seen(node)
    }

    fn update_ssn(&mut self, node: NodeId, ssn: SafeStateSeqNr, now: Instant) -> bool {
        self.inner.update_ssn(node, ssn, now)
    }
    fn ssn_vicinity(&self, node: &NodeId) -> Option<SafeStateSeqNr> {
        self.inner.ssn_vicinity(node)
    }

    fn force_resync(&mut self, node: &NodeId) {
        self.inner.force_resync(node);
    }

    fn requires_resync(&self, node: &NodeId) -> bool {
        self.inner.requires_resync(node)
    }

    fn resync_nodes(&self) -> impl Iterator<Item = &NodeId> {
        self.inner.resync_nodes()
    }
}
