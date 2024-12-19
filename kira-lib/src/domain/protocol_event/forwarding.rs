//! Events for interaction with the fast forwarding functionality of KIRA.

use derive_more::derive::Display;

use crate::domain::{NodeId, NodeIdSubnet, PathId, UnderlayNeighborId};

/// An update request to the fast forwarding functionality.
#[derive(Debug, Clone, Display)]
pub enum ForwardingTablesUpdate {
    NodeIdTableUpdate(NodeIdTableUpdate),
    PathIdTableUpdate(PathIdTableUpdate),
}

/// An update request to the [NodeId]-table.
#[derive(Debug, Clone, Display)]
pub enum NodeIdTableUpdate {
    Create(NodeIdEntry),
    Update(NodeIdEntry),
    CreateOrUpdate(NodeIdEntry),
    Remove(NodeIdSubnet),
}

/// An update request to the [PathId]-table.
#[derive(Debug, Clone, Display)]
pub enum PathIdTableUpdate {
    Create(PathIdEntry),
    Update(PathIdEntry),
    CreateOrUpdate(PathIdEntry),
    Remove(PathId),
}

/// An entry in the [NodeIdTable] identified by the destination/contacts [NodeIdSubnet].
#[derive(Debug, Clone, Display)]
pub enum NodeIdEntry {
    Forward(NodeIdForwardingEntry),
    Encapsulate(NodeIdEncapsulationEntry),
}

/// An entry in the [NodeIdTable] for forwarding packets to a physical neighbor.
#[derive(Debug, Clone, Display)]
#[display("{destination} => {next_hop}")]
pub struct NodeIdForwardingEntry {
    /// [NodeIdSubnet] of the contact this entry goes to.
    pub destination: NodeIdSubnet,
    /// [UnderlayNeighborId] of the next node to pass the data packet to.
    pub next_hop: UnderlayNeighborId,
}

/// An entry in the [NodeIdTable] for encapsulating packets to a contact.
#[derive(Debug, Clone, Display)]
#[display("{destination} => {next_hop} ({out_path_id}")]
pub struct NodeIdEncapsulationEntry {
    /// [NodeIdSubnet] of the contact this entry goes to.
    pub destination: NodeIdSubnet,
    /// [PathId] to use for forwarding
    pub out_path_id: PathId,
    /// [UnderlayNeighborId] of the next node to pass the data packet to.
    pub next_hop: UnderlayNeighborId,
}

/// An entry in the [PathIdEntry] identified by incoming [PathId] (with current nodes [NodeId]).
#[derive(Debug, Clone, Display)]
pub enum PathIdEntry {
    Forward(PathIdForwardingEntry),
    Decapsulate(PathIdDecapsulationEntry),
}

/// An entry in the [PathIdTable] for forwarding packets.
#[derive(Debug, Clone, Display)]
#[display("{in_path_id} => {out_path_id} ({next_hop})")]
pub struct PathIdForwardingEntry {
    /// [PathId] of incoming packages starting with the [NodeId] of the current node.
    pub in_path_id: PathId,
    /// [PathId] of the outgoing packages (in_path_id will be replaced by this) not starting
    /// with the [NodeId] of the current node.
    pub out_path_id: PathId,
    /// [UnderlayNeighborId] of the next node to pass the data packet to.
    pub next_hop: UnderlayNeighborId,
}

/// An entry in the [PathIdTable] for decapsulating packets.
#[derive(Debug, Clone, Display)]
#[display("{in_path_id} => None ({local_id})")]
pub struct PathIdDecapsulationEntry {
    /// [PathId] of incoming packages starting with the [NodeId] of the current node.
    pub in_path_id: PathId,
    /// [NodeId] of a local node.
    pub local_id: NodeId,
}
