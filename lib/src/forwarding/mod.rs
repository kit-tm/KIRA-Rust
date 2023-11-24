//! Type definitions for the forwarding layer interface.

use std::fmt::{Debug, Display, Formatter};

use crate::domain::path_id::PathId;
use crate::domain::{NetworkInterface, NodeId};

pub mod hasher;
pub mod in_memory_tables;
pub mod native_tables;

/// An entry in the [NodeIdTable] identified by the destination/contacts [NodeId].
#[derive(Debug, Eq, PartialEq, Clone, Hash)]
pub struct NodeIdEntry {
    /// [NodeId] of the contact this entries path goes to.
    pub destination: NodeId,
    /// length of the prefix if destination represents a prefix, 0 otherwise.
    pub prefix_len: usize,
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

impl Display for NodeIdEntry {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.destination, f)?;
        write!(f, "/")?;
        Display::fmt(&self.prefix_len, f)?;
        write!(f, " => (")?;
        Display::fmt(&self.next_hop, f)?;
        write!(f, ", ")?;
        if let Some(id) = &self.out_path_id {
            Display::fmt(id, f)?;
        } else {
            write!(f, "-")?;
        }
        write!(f, ", ")?;
        Display::fmt(&self.out_interface, f)?;
        write!(f, ")")
    }
}

/// An entry in the [PathIdEntry] identified by incoming [PathId] (with current nodes [NodeId]).
#[derive(Debug, Eq, PartialEq, Clone, Hash)]
pub struct PathIdEntry {
    /// [PathId] of incoming packages starting with the [NodeId] of the current node.
    pub in_path_id: PathId,
    /// [PathId] of the outgoing packages (in_path_id will be replaced by this) not starting
    /// with the [NodeId] of the current node. If this is None packets will be decapsulated.
    pub out_path_id: Option<PathId>,
    /// [NodeId] to delegate the data packet to.
    pub next_hop: NodeId,
}

impl Display for PathIdEntry {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.in_path_id, f)?;
        write!(f, " => (")?;
        if let Some(out_path_id) = &self.out_path_id {
            Display::fmt(out_path_id, f)?;
        } else {
            Display::fmt("None", f)?;
        }
        write!(f, ", ")?;
        Display::fmt(&self.next_hop, f)?;
        write!(f, ")")
    }
}

/// CRUD access interface to the forwarding table based on [NodeId]s.
pub trait NodeIdTable {
    type Error: Debug;

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
    type Error: Debug;

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
