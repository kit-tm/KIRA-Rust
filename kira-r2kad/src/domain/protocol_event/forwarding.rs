//! Events for interaction with the fast forwarding functionality of KIRA.

use derive_more::derive::{
    Display,
    From,
};

use crate::domain::{
    NodeIdSubnet,
    PathId,
    UnderlayNeighborId,
};

/// An update request to the fast forwarding functionality.
#[derive(Debug, Clone, Display, From)]
pub enum ForwardingTablesUpdate {
    /// Update request to the NodeIdForwardingTable.
    ///
    /// The NodeIdForwardingTable consists out of [NodeIdEntrys](NodeIdEntry).
    NodeIdTableUpdate(NodeIdTableUpdate),
    /// Update request to the [PathId]-forwarding-table.
    ///
    /// The PathIdForwardingTable consists out of [PathIdEntrys](PathIdEntry).
    PathIdTableUpdate(PathIdTableUpdate),
}

/// An update request to the NodeIdForwardingTable.
#[derive(Debug, Clone, Display)]
#[display("{_variant}: {_0}")]
pub enum NodeIdTableUpdate {
    /// Creates the given [NodeIdEntry].
    ///
    /// Should fail if an entry with [destination](NodeIdEntry::destination) already exists.
    Create(NodeIdEntry),
    /// Updates the given [NodeIdEntry].
    ///
    /// Should fail if no entry with [destination](NodeIdEntry::destination) does exist yet.
    Update(NodeIdEntry),
    /// Creates the given [NodeIdEntry] if it doesn't exist yet, otherwise updates it.
    CreateOrUpdate(NodeIdEntry),
    /// Removes a [NodeIdEntry].
    ///
    /// Should *not* fail if no entry with [destination](NodeIdEntry::destination) exists.
    Remove(NodeIdSubnet),
}

/// An update request to the PathIdForwardingTable.
#[derive(Debug, Clone, Display)]
#[display("{_variant}: {_0}")]
pub enum PathIdTableUpdate {
    /// Creates the given [PathIdEntry].
    ///
    /// Should fail if an entry with [in_path_id](PathIdEntry::in_path_id) already exists.
    Create(PathIdEntry),
    /// Updates the given [PathIdEntry].
    ///
    /// Should fail if no entry with [in_path_id](PathIdEntry::in_path_id) does exist yet.
    Update(PathIdEntry),
    /// Creates the given [PathIdEntry] if it doesn't exist yet, otherwise updates it.
    CreateOrUpdate(PathIdEntry),
    /// Removes a [PathIdEntry].
    ///
    /// Should *not* fail if no entry with [in_path_id](PathIdEntry::in_path_id) exists.
    Remove(PathId),
}

/// An entry in the NodeIdTable identified by the destination/contacts [NodeIdSubnet].
#[derive(Debug, Clone, Display, PartialEq, Eq, Hash)]
pub enum NodeIdEntry {
    /// Forward the packets to a underlay neighbor.
    Forward(NodeIdForwardingEntry),
    /// Encapsulate packets with a [PathId].
    Encapsulate(NodeIdEncapsulationEntry),
}

impl NodeIdEntry {
    /// Destination of the [NodeIdEntry].
    pub fn destination(&self) -> &NodeIdSubnet {
        match self {
            NodeIdEntry::Forward(NodeIdForwardingEntry { destination, .. }) => destination,
            NodeIdEntry::Encapsulate(NodeIdEncapsulationEntry { destination, .. }) => destination,
        }
    }

    /// [PathId] that is used as temporary destination on encapsulation.
    pub fn out_path_id(&self) -> Option<&PathId> {
        match self {
            NodeIdEntry::Forward(_) => None,
            NodeIdEntry::Encapsulate(NodeIdEncapsulationEntry { out_path_id, .. }) => {
                Some(out_path_id)
            }
        }
    }
}

/// An entry in the NodeIdTable for forwarding packets to a underlay neighbor.
#[derive(Debug, Clone, Display, PartialEq, Eq, Hash)]
#[display("{destination} => {next_hop}")]
pub struct NodeIdForwardingEntry {
    /// [NodeIdSubnet] of the contact this entry goes to.
    pub destination: NodeIdSubnet,
    /// [UnderlayNeighborId] of the next node to pass the data packet to.
    pub next_hop: UnderlayNeighborId,
}

/// An entry in the NodeIdTable for encapsulating packets to a contact.
#[derive(Debug, Clone, Display, PartialEq, Eq, Hash)]
#[display("{destination} => {next_hop} ({out_path_id})")]
pub struct NodeIdEncapsulationEntry {
    /// [NodeIdSubnet] of the contact this entry goes to.
    pub destination: NodeIdSubnet,
    /// [PathId] to use for forwarding
    pub out_path_id: PathId,
    /// [UnderlayNeighborId] of the next underlay hop to forward the data packet to.
    pub next_hop: UnderlayNeighborId,
}

/// An entry in the PathIdTable identified by incoming [PathId] (with current nodes
/// [NodeId](crate::domain::NodeId)).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Display)]
pub enum PathIdEntry {
    /// Forward packets to underlay neighbor.
    Forward(PathIdForwardingEntry),
    /// Decapsulate packets by removing the [PathId].
    Decapsulate(PathIdDecapsulationEntry),
}

impl PathIdEntry {
    /// Expected [PathId] destination on ingress.
    pub fn in_path_id(&self) -> &PathId {
        match self {
            PathIdEntry::Forward(PathIdForwardingEntry { in_path_id, .. }) => in_path_id,
            PathIdEntry::Decapsulate(PathIdDecapsulationEntry { in_path_id, .. }) => in_path_id,
        }
    }

    /// [PathId] that is used as temporary destination on encapsulation.
    pub fn out_path_id(&self) -> Option<&PathId> {
        match self {
            PathIdEntry::Forward(PathIdForwardingEntry { out_path_id, .. }) => Some(out_path_id),
            PathIdEntry::Decapsulate(_) => None,
        }
    }
}

/// An entry in the PathId-table for forwarding packets.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Display)]
#[display("{in_path_id} => {out_path_id} ({next_hop})")]
pub struct PathIdForwardingEntry {
    /// [PathId] of incoming packages starting with the [NodeId](crate::domain::NodeId) of the current node.
    pub in_path_id: PathId,
    /// [PathId] of the outgoing packages (in_path_id will be replaced by this) not starting
    /// with the [NodeId](crate::domain::NodeId) of the current node.
    pub out_path_id: PathId,
    /// [UnderlayNeighborId] of the next underlay hop to forward the data packet to.
    pub next_hop: UnderlayNeighborId,
}

/// An entry in the PathIdTable for decapsulating packets.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Display)]
#[display("{in_path_id} => None ({next_hop})")]
pub struct PathIdDecapsulationEntry {
    /// [PathId] of incoming packages starting with the [NodeId](crate::domain::NodeId) of the current node.
    pub in_path_id: PathId,
    /// [DecapsulationDestination] on where to forward the data packet to.
    pub next_hop: DecapsulationDestination,
}

/// Destination of [decapsulated](PathIdDecapsulationEntry) packets.
#[derive(Debug, Display, Eq, PartialEq, Clone, Hash)]
pub enum DecapsulationDestination {
    /// The final destination is the current node.
    ///
    /// This will abort any further forwarding and attempts
    /// to locally deliver the packet.
    ///
    /// # Note
    ///
    /// If *penultimate hop popping* is employed this
    /// entry is still required for successfully delivering
    /// packets of penultimate hops not employing the
    /// penultimate hop popping mechanism.
    #[display("local")]
    Local,
    /// [UnderlayNeighborId] of the next underlay hop to forward the data packet to.
    ///
    /// This is used if the forwarding layer is employing *penultimate hop popping*.
    /// *Penultimate hop popping* will decapsulate the packet the hop before reaching
    /// its final hop on the forwarding path.
    UnderlayNeighbor(UnderlayNeighborId),
}
