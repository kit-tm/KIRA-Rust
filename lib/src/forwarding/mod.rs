use crate::domain::path_id::PathId;
use crate::domain::{NetworkInterface, NodeId};

pub mod hasher;
pub mod in_memory_tables;

/// An entry in the [NodeIdTable] identified by the destination/contacts [NodeId].
#[derive(Debug, Eq, PartialEq)]
pub struct NodeIdEntry {
    /// [NodeId] of the contact this entries path goes to.
    pub destination: NodeId,
    /// [NodeId] of the next node to pass the data packet to.
    ///
    /// Is a physical neighbor of the current node.
    pub next_hop: NodeId,
    /// [PathId] to use for redirection.
    ///
    /// If the contact this entry belongs to is a physical neighbor, this will be [None].
    pub out_path_id: Option<PathId>,
    /// [NetworkInterface] to delegate the data packet to.
    pub out_interface: NetworkInterface,
}

/// An entry in the [PathIdEntry] identified by incoming [PathId] (with current nodes [NodeId]).
#[derive(Debug, Eq, PartialEq)]
pub struct PathIdEntry {
    /// [PathId] of incoming packages starting with the [NodeId] of the current node.
    pub in_path_id: PathId,
    /// [PathId] of the outgoing packages (in_path_id will be replaced by this) not starting
    /// with the [NodeId] of the current node.
    pub out_path_id: PathId,
    /// [NetworkInterface] to delegate the data packet to.
    pub out_interface: NetworkInterface,
}

/// CRUD access interface to the forwarding table based on [NodeId]s.
pub trait NodeIdTable {
    type Error;

    /// Creates the given [NodeIdEntry].
    ///
    /// Emits an error if an entry with the entries `destination` already exists.
    fn create(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error>;
    /// Updates an existing [NodeIdEntry].
    ///
    /// Emits an error if the entries destination [NodeId] doesn't yet exist.
    fn update(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error>;
    /// Removes a [NodeIdEntry].
    ///
    /// Doesn't emit an error if the entries destination [NodeId] doesn't exist.
    ///
    /// Returns the removed [NodeIdEntry].
    fn remove(&mut self, node_id: &NodeId) -> Result<Option<NodeIdEntry>, Self::Error>;
}

/// CRUD access interface to the forwarding table based on [PathId]s.
pub trait PathIdTable {
    type Error;

    /// Creates the given [PathIdEntry].
    ///
    /// Emits an error if an entry with the entries `in_path_id` already exists.
    fn create(&mut self, entry: PathIdEntry) -> Result<(), Self::Error>;
    /// Updates an existing [PathIdEntry].
    ///
    /// Emits an error if the entries `in_path_id` doesn't yet exist.
    fn update(&mut self, entry: PathIdEntry) -> Result<(), Self::Error>;
    /// Removes a [PathIdEntry].
    ///
    /// **Doesn't** emit an error if the entries `in_path_id` doesn't exist.
    ///
    /// Returns the removed [PathIdEntry].
    fn remove(&mut self, path_id: &PathId) -> Result<Option<PathIdEntry>, Self::Error>;
}

/// CRUD access interface to the whole forwarding layer and its forwarding tables.
pub trait ForwardingTables: NodeIdTable + PathIdTable {}
