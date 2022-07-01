use std::fmt::{Display, Formatter};

use crate::domain::StateSeqNr;

use super::NodeId;

pub mod neighbor_hash_table;

/// Represents a logical [Port] where a [Message] can be received from or sent to.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Port {
    All,
    Named(String),
}

impl Port {
    pub fn new(id: String) -> Self {
        Self::Named(id)
    }
}

impl Display for Port {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::All => write!(f, "All"),
            Self::Named(name) => write!(f, "Port [\"{}\"]", name),
        }
    }
}

/// Maps [NodeId]s to [Port]s.
pub trait NeighborTable {
    /// Returns the [Port] for a [NodeId] if present.
    fn get(&self, id: &NodeId) -> Option<&Port>;
    /// Adds a Mapping to the table returning the [Port] previously mapped to the [NodeId].
    fn add(&mut self, id: NodeId, iface: Port) -> Option<Port>;
    /// Returns if a Mapping for the [NodeId] is present in the [NeighborTable].
    fn contains(&self, id: &NodeId) -> bool;
    /// Returns if any neighbors are present.
    fn is_empty(&self) -> bool;
    /// Returns the number of neighbors.
    fn len(&self) -> usize;
    /// Returns the state sequence number.
    ///
    /// The state sequence number represents the number of connectivity changes in the
    /// direct physical neighborhood of a node.
    fn state_seq_nr(&self) -> &StateSeqNr;
}
