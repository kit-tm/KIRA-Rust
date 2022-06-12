use super::{StateSeqNr, Age, NodeId};

/// Value for Entries for the [DiscoveryTable] containing [Contact] information without the [Path].
pub struct DiscoveryData<const ID_SIZE: usize> {
    pub state_seq_nr: StateSeqNr,
    pub age: Age,
}

/// A [DiscoveryTable] contains information about a [Contact] whose [Path]
/// is not yet discovered.
pub trait DiscoveryTable<const ID_SIZE: usize> {
    /// Adds a [DiscoveryData] to the [DiscoveryTable] returning the previous
    /// [DiscoveryData] for the [NodeId] if present.
    fn add(&self, id: NodeId<ID_SIZE>, entry: DiscoveryData<ID_SIZE>) -> Option<DiscoveryData<ID_SIZE>>;
    /// Removes [DiscoveryData] from the [DiscoveryTable] by [NodeId] and returns it.
    fn remove(&mut self, id: &NodeId<ID_SIZE>) -> Option<DiscoveryData<ID_SIZE>>;
}