//! Definition of the [VicinityGraph].

pub mod entry;
pub mod observable_vicinity_graph;
pub mod pet_vicinity_graph;

pub use entry::Entry;
pub use observable_vicinity_graph::ObservableVicinityGraph;
pub use pet_vicinity_graph::PetVicinityGraph;

use crate::domain::{NodeId, Path, SafeStateSeqNr};
use std::error::Error;

pub trait VicinityGraph {
    type Error: Error;

    //  === INDIVIDUAL NODES MANAGEMENT ===

    /// Inserts a node as a neighbor of `discovered_via` into the [VicinityGraph].
    ///
    /// The function returns if a new edge and entry where created by the insertion.
    ///
    /// If the node already exists only the `observed_ssn` of the [Entry] is updated.
    /// An error is returned if the node would make the [VicinityGraph] unconnected.
    fn insert(
        &mut self,
        node: NodeId,
        discovered_via: &NodeId,
        observed_ssn: SafeStateSeqNr,
    ) -> Result<bool, Self::Error>;

    /// Removes a `node` from the vicinity graph.
    ///
    /// Returns if the vicinity graph changed by the removal.
    fn remove(&mut self, node: &NodeId) -> bool;

    /// Returns the [Entry] of the node if present.
    fn entry(&self, node: &NodeId) -> Option<&Entry>;

    /// Returns the mutable [Entry] of the node if present.
    fn entry_mut(&mut self, node: &NodeId) -> Option<&mut Entry>;

    /// Returns the current vicinity (neighborhood) of the node.
    ///
    /// If the node doesn't exist an empty iterator is returned.
    fn vicinity(&self, node: &NodeId) -> impl Iterator<Item = NodeId>;

    /// Returns the distance of the node to the root of the [VicinityGraph] if present.
    fn root_distance(&self, node: &NodeId) -> Option<usize>;

    //  === VICINITY GRAPH ACCESS ===

    /// Removes an edge from the vicinity graph.
    ///
    /// Returns if the vicinity graph was altered.
    fn remove_edge(&mut self, node_a: &NodeId, node_b: &NodeId) -> bool;

    /// Only keeps nodes that are inside the vicinity radius.
    ///
    /// Returns an iterator over all removed nodes.
    ///
    /// Nodes can only be [inserted](VicinityGraph::insert) if
    /// they are in the vicinity radius but an update of its
    /// vicinity could change it to outside of the vicinity.
    fn retain_vicinity(&mut self) -> impl Iterator<Item = NodeId>;

    // Returns a list of all nodes considered in the vicinity.
    //
    // The method can return nodes that where previously in the vicinity
    // but moved outside the vicinity.
    // If you want to ensure that only nodes inside the vicinity are returned
    // call [`retain_radius`] first.
    //
    // [`retain_radius`]: VicinityGraph::retain_radius
    fn nodes(&self) -> impl Iterator<Item = NodeId>;

    /// Computes *all* paths from the root to its vicinity nodes.
    ///
    /// The paths must inside the vicinity.
    fn vicinity_paths(&self) -> impl Iterator<Item = Path>;

    /// Computes **all* paths from the root to the destination.
    ///
    /// The paths must be inside the vicinity.
    fn vicinity_paths_to(&self, destination: NodeId) -> impl Iterator<Item = Path> {
        self.vicinity_paths()
            .filter(move |path| path.first() == &destination)
    }

    /// Computes the shortest path from the root to the destination.
    fn vicinity_path_to(&self, destination: NodeId) -> Option<Path> {
        self.vicinity_paths_to(destination)
            .min_by_key(|path| path.size())
    }

    /// returns whether the vicinity has been changed since last time the vicinity_processed was called
    fn vicinity_changed(&self) -> bool;
    /// this should be called if precomputed paths have been calculated
    fn vicinity_processed(&mut self);
}

#[cfg(test)]
pub mod test {}
