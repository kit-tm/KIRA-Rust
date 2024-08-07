//! Type definitions for the forwarding layer interface.

use std::fmt::{Debug, Display, Formatter};

use crate::domain::path_id::PathId;
use crate::domain::{NetworkInterface, NodeId, NodeIdSubnet};

pub use platform::*;

pub mod hasher;
pub mod in_memory_tables;
pub mod native_tables;
pub mod platform;

/// An entry in the [NodeIdTable] identified by the destination/contacts [NodeIdSubnet].
#[derive(Debug, Eq, PartialEq, Clone, Hash)]
pub enum NodeIdEntry {
    Forward(NodeIdForwardingEntry),
    Encapsulate(NodeIdEncapsulationEntry),
}

impl Display for NodeIdEntry {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeIdEntry::Forward(entry) => Display::fmt(entry, f),
            NodeIdEntry::Encapsulate(entry) => Display::fmt(entry, f),
        }
    }
}

/// An entry in the [NodeIdTable] for forwarding packets to a physical neighbor.
#[derive(Debug, Eq, PartialEq, Clone, Hash)]
pub struct NodeIdForwardingEntry {
    /// [NodeIdSubnet] of the contact this entry goes to.
    pub destination: NodeIdSubnet,
    /// [NodeId] of the next node to pass the data packet to.
    pub next_hop: NodeId,
    /// [NetworkInterface] to delegate the data packet to.
    pub out_interface: NetworkInterface,
}

impl Display for NodeIdForwardingEntry {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.destination, f)?;
        write!(f, " => ")?;
        Display::fmt(&self.next_hop, f)?;
        write!(f, " ({})", self.out_interface)
    }
}

/// An entry in the [NodeIdTable] for encapsulating packets to a contact.
#[derive(Debug, Eq, PartialEq, Clone, Hash)]
pub struct NodeIdEncapsulationEntry {
    /// [NodeIdSubnet] of the contact this entry goes to.
    pub destination: NodeIdSubnet,
    /// [NodeId] of the next node to pass the data packet to.
    pub next_hop: NodeId,
    /// [PathId] to use for forwarding
    pub out_path_id: PathId,
    /// [NetworkInterface] to delegate the data packet to.
    pub out_interface: NetworkInterface,
}

impl Display for NodeIdEncapsulationEntry {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.destination, f)?;
        write!(f, " => ")?;
        Display::fmt(&self.next_hop, f)?;
        write!(f, " (")?;
        Display::fmt(&self.out_path_id, f)?;
        write!(f, ", {})", self.out_interface)
    }
}

/// An entry in the [PathIdEntry] identified by incoming [PathId] (with current nodes [NodeId]).
#[derive(Debug, Eq, PartialEq, Clone, Hash)]
pub enum PathIdEntry {
    Forward(PathIdForwardingEntry),
    Decapsulate(PathIdDecapsulationEntry),
}

impl Display for PathIdEntry {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            PathIdEntry::Forward(entry) => Display::fmt(entry, f),
            PathIdEntry::Decapsulate(entry) => Display::fmt(entry, f),
        }
    }
}

/// An entry in the [PathIdTable] for forwarding packets.
#[derive(Debug, Eq, PartialEq, Clone, Hash)]
pub struct PathIdForwardingEntry {
    /// [PathId] of incoming packages starting with the [NodeId] of the current node.
    pub in_path_id: PathId,
    /// [PathId] of the outgoing packages (in_path_id will be replaced by this) not starting
    /// with the [NodeId] of the current node.
    pub out_path_id: PathId,
    /// [NodeId] to delegate the data packet to.
    pub next_hop: NodeId,
}

impl Display for PathIdForwardingEntry {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.in_path_id, f)?;
        write!(f, " => ")?;
        Display::fmt(&self.out_path_id, f)?;
        write!(f, "({})", self.next_hop)
    }
}

/// An entry in the [PathIdTable] for decapsulating packets.
#[derive(Debug, Eq, PartialEq, Clone, Hash)]
pub struct PathIdDecapsulationEntry {
    /// [PathId] of incoming packages starting with the [NodeId] of the current node.
    pub in_path_id: PathId,
    /// [NodeId] of a local node.
    pub local_id: NodeId,
}

impl Display for PathIdDecapsulationEntry {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.in_path_id, f)?;
        write!(f, " => None")
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
    /// Creates the given [NodeIdEntry] if it doesn't exist yet, otherwise updates it.
    fn create_or_update(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error>;
    /// Removes a [NodeIdEntry].
    ///
    /// Doesn't emit an error if the entries destination [NodeId] doesn't exist.
    ///
    /// Returns the removed [NodeIdEntry].
    fn remove(&mut self, node_id: &NodeIdSubnet) -> Result<Option<NodeIdEntry>, Self::Error>;
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
    /// Creates the given [PathIdEntry] if it doesn't exist yet, otherwise updates it.
    fn create_or_update(&mut self, entry: PathIdEntry) -> Result<(), Self::Error>;
    /// Removes a [PathIdEntry].
    ///
    /// **Doesn't** emit an error if the entries `in_path_id` doesn't exist.
    ///
    /// Returns the removed [PathIdEntry].
    fn remove(&mut self, path_id: &PathId) -> Result<Option<PathIdEntry>, Self::Error>;
}

/// CRUD access interface to the whole forwarding layer and its forwarding tables.
pub trait ForwardingTables: NodeIdTable + PathIdTable {}
