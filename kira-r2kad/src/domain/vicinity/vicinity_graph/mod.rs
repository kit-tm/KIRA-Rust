//! Definition of the [VicinityGraph].

pub mod observable_vicinity_graph;
pub mod pet_vicinity_grah;

pub use observable_vicinity_graph::ObservableVicinityGraph;
pub use pet_vicinity_grah::PetVicinityGraph;

use crate::domain::{NodeId, Path, SafeStateSeqNr};
use std::{error::Error, time::Instant};

pub trait VicinityGraph {
    type Error: Error;

    fn insert(
        &mut self,
        node: NodeId,
        neighbors: impl IntoIterator<Item = NodeId>,
        ssn: SafeStateSeqNr,
        last_seen: Instant,
    ) -> Result<(), Self::Error>;

    /// Removes a `node` from the vicinity graph.
    ///
    /// This will keep the meta-data of the `node`.
    ///
    /// Returns if the vicinity graph changed by the removal.
    fn remove(&mut self, node: &NodeId) -> bool;

    /// Removes a node from the vicinity graph _and_ deletes its meta-data.
    ///
    /// Returns if the vicinity graph changed by the removal.
    fn prune(&mut self, node: &NodeId) -> bool;

    /// Remove all nodes that are outside the [VICINITY_RADIUS].
    ///
    /// This function will _not_ delete the associated Entry of a pruned node.
    fn remove_radius(&mut self) -> bool;

    // TODO: document
    fn nodes(&self) -> impl Iterator<Item = &NodeId>;

    /// Computes *all* paths from the root to it's vicinity nodes.
    ///
    /// The paths must not be longer then [VICINITY_RADIUS](super::VICINITY_RADIUS).
    fn paths(&self) -> impl Iterator<Item = Path>;

    /// Computes **all* paths from the root to the destination.
    ///
    /// The paths must not be longer then [VICINITY_RADIUS](super::VICINITY_RADIUS).
    fn paths_to(&self, destination: NodeId) -> impl Iterator<Item = Path> {
        self.paths()
            .filter(move |path| path.first() == &destination)
    }

    /// Updates the time a node was last seen.
    ///
    /// Returns if the time was updated.
    fn update_last_seen(&mut self, node: &NodeId, now: Instant) -> bool;

    /// Returns last time a contact with the `node` was made.
    fn last_seen(&self, node: &NodeId) -> Option<Instant>;

    /// Updates the newest known [SafeStateSeqNr].
    ///
    /// Returns if the [SafeStateSeqNr] was newer than the previous known [SafeStateSeqNr].
    ///
    /// The newest known [SafeStateSeqNr] is used to determine if the [VicinityGraph]
    /// has the newest state of the vicinity of said node saved.
    /// The function *always* updates the last seen state.
    fn update_ssn(&mut self, node: NodeId, ssn: SafeStateSeqNr, now: Instant) -> bool;

    /// Return the state sequence number of the `node`.
    ///
    /// The vicinity state described with the returned state sequence number
    /// is the last known state by the [VicinityGraph].
    /// The state is also applied in the [VicinityGraph].
    fn ssn_vicinity(&self, node: &NodeId) -> Option<SafeStateSeqNr>;

    /// Marks the `node` to be resynchronised.
    ///
    /// Nodes not known previously will *not* be marked for resynchronisation.
    /// *Any* vicinity information received by the `node` will complete the resync
    /// unless a newer SSN was observed and updated.
    /// See [`update_ssn`](VicinityGraph::update_ssn).
    fn force_resync(&mut self, node: &NodeId);

    /// Returns if the `node` must be resynced.
    ///
    /// Usually this is because vicinity information of a node is deemed outdated
    /// and must be updated to the newest state.
    /// A resync can also be forced using [`force_resync`](VicinityGraph::force_resync).
    fn requires_resync(&self, node: &NodeId) -> bool;

    /// Lists all nodes that require resynchronisation.
    ///
    /// See [`requires_resync`](VicinityGraph::requires_resync) why this would be the case
    fn resync_nodes(&self) -> impl Iterator<Item = &NodeId> {
        self.nodes().filter(|node| self.requires_resync(node))
    }
}
