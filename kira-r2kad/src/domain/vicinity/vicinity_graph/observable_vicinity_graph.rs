use std::fmt::Debug;

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
        discovered_via: &NodeId,
        observed_ssn: SafeStateSeqNr,
    ) -> Result<bool, Self::Error> {
        self.inner.insert(node, discovered_via, observed_ssn)
    }

    fn remove(&mut self, node: &NodeId) -> bool {
        if self.inner.remove(node) {
            self.emit(VicinityGraphEvent::Removed(*node));
            true
        } else {
            false
        }
    }

    fn entry(&self, node: &NodeId) -> Option<&super::Entry> {
        self.inner.entry(node)
    }

    fn entry_mut(&mut self, node: &NodeId) -> Option<&mut super::Entry> {
        self.inner.entry_mut(node)
    }

    fn vicinity(&self, node: &NodeId) -> impl Iterator<Item = NodeId> {
        self.inner.vicinity(node)
    }

    fn root_distance(&self, node: &NodeId) -> Option<usize> {
        self.inner.root_distance(node)
    }

    fn remove_edge(&mut self, node_a: &NodeId, node_b: &NodeId) -> bool {
        self.inner.remove_edge(node_a, node_b)
    }

    fn retain_vicinity(&mut self) -> impl Iterator<Item = NodeId> {
        let removed_nodes: Vec<_> = self.inner.retain_vicinity().collect();
        for removed in removed_nodes.iter() {
            self.emit(VicinityGraphEvent::Removed(*removed));
        }

        removed_nodes.into_iter()
    }

    fn nodes(&self) -> impl Iterator<Item = NodeId> {
        self.inner.nodes()
    }

    fn vicinity_paths(&self) -> impl Iterator<Item = Path> {
        self.inner.vicinity_paths()
    }

    fn vicinity_paths_to(&self, destination: NodeId) -> impl Iterator<Item = Path> {
        self.inner.vicinity_paths_to(destination)
    }

    fn vicinity_path_to(&self, destination: NodeId) -> Option<Path> {
        self.inner.vicinity_path_to(destination)
    }

    fn vicinity_changed(&self) -> bool {
        self.inner.vicinity_changed()
    }

    fn vicinity_processed(&mut self) {
        self.inner.vicinity_processed();
    }
}
