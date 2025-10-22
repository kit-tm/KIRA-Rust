//! Definition of the [VicinityGraph].

pub mod entry;
pub mod observable_vicinity_graph;
pub mod pet_vicinity_graph;

pub use entry::Entry;
pub use observable_vicinity_graph::ObservableVicinityGraph;
pub use pet_vicinity_graph::PetVicinityGraph;

use crate::domain::{NodeId, Path, SafeStateSeqNr};
use std::{error::Error, time::Instant};

pub trait VicinityGraph {
    type Error: Error;

    // FIXME: we need to store information about pending 3 hop syncs

    //  === INDIVIDUAL NODES MANAGEMENT ===

    /// Inserts a node into the [VicinityGraph].
    ///
    /// An error is returned if the node would make the [VicinityGraph] unconnected.
    fn insert(
        &mut self,
        node: NodeId,
        discovered_via: &NodeId,
        observed_ssn: SafeStateSeqNr,
    ) -> Result<(), Self::Error>;

    /// Updates the vicinity of the node.
    ///
    /// A change of the nodes vicinity must leave the
    /// node connected to the root of the [VicinityGraph]
    /// otherwise an error is returned.
    fn update_vicinity(
        &mut self,
        node: &NodeId,
        neighbors: impl IntoIterator<Item = NodeId>,
        vicinity_ssn: SafeStateSeqNr,
    ) -> Result<(), Self::Error>;

    /// Resets the known vicinity of a node.
    ///
    /// In contrast to [`update_vicinity`] this
    /// function doesn't alter the actual stored vicinity information
    /// but only resets the [`observed_ssn`] and the [`vicinity_ssn`].
    ///
    /// [`update_vicinity`]: VicinityGraph::update_vicinity
    /// [`observed_ssn`]: VicinityGraph::observed_ssn
    /// [`vicinity_ssn`]: VicinityGraph::vicinity_ssn
    fn reset(&mut self, node: &NodeId);

    /// Removes a `node` from the vicinity graph.
    ///
    /// Returns if the vicinity graph changed by the removal.
    fn remove(&mut self, node: &NodeId) -> bool;

    /// Updates the time a node was last seen.
    fn update_last_seen(&mut self, node: &NodeId, now: Instant);

    /// Updates the greatest observed state sequence number.
    fn update_observed_ssn(&mut self, node: &NodeId, observed_ssn: SafeStateSeqNr);

    //  === INDIVIDUAL NODES META-DATA QUERY ===

    /// Returns last time a contact with the `node` was made.
    fn last_seen(&self, node: &NodeId) -> Option<Instant>;

    /// Greatest observed state sequence number.
    ///
    /// The observed state sequence number can decrease on direct contact to a node.
    /// This is usually because of a reset initiated by the node caused by reaching
    /// the maximum state sequence number.
    fn observed_ssn(&self, node: &NodeId) -> Option<&SafeStateSeqNr>;

    /// Returns the current vicinity (neighborhood) of the node.
    ///
    /// If the node doesn't exist an empty iterator is returned.
    fn vicinity(&self, node: &NodeId) -> impl Iterator<Item = NodeId>;

    /// State sequence number of the vicinity present in the [VicinityGraph].
    ///
    /// The value can be [`None`] if the vicinity state of the node hasn't been acquired
    /// either because a synchronisation is ongoing or because of a node reset initialised.
    fn vicinity_ssn(&self, node: &NodeId) -> Option<&SafeStateSeqNr>;

    /// Returns the distance of the node to the root of the [VicinityGraph] if present.
    fn root_distance(&self, node: &NodeId) -> Option<usize>;

    //  === VICINITY GRAPH ACCESS ===

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

    /// Computes *all* paths from the root to it's vicinity nodes.
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
}

#[cfg(test)]
pub mod test {}
