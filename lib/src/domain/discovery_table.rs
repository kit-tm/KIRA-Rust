use std::collections::HashMap;

use super::{Age, NodeId, StateSeqNr};

/// Value for Entries for the [DiscoveryTable] containing [Contact] information without the [Path].
pub struct DiscoveryData {
    pub state_seq_nr: StateSeqNr,
    pub age: Age,
}

/// A [DiscoveryTable] contains information about a [Contact] whose [Path]
/// is not yet discovered.
pub trait DiscoveryTable {
    /// Adds a [DiscoveryData] to the [DiscoveryTable] returning the previous
    /// [DiscoveryData] for the [NodeId] if present.
    fn add(&mut self, id: NodeId, entry: DiscoveryData) -> Option<DiscoveryData>;
    /// Removes [DiscoveryData] from the [DiscoveryTable] by [NodeId] and returns it.
    fn remove(&mut self, id: &NodeId) -> Option<DiscoveryData>;
}

impl DiscoveryTable for HashMap<NodeId, DiscoveryData> {
    fn add(&mut self, id: NodeId, entry: DiscoveryData) -> Option<DiscoveryData> {
        self.insert(id, entry)
    }

    fn remove(&mut self, id: &NodeId) -> Option<DiscoveryData> {
        self.remove(id)
    }
}
