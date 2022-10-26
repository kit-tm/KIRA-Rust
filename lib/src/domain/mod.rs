use std::fmt::{Display, Formatter};

// To change default NodeId simply change this
pub use bucket::*;
pub use contact::*;
pub use insertion_strategy::*;
pub use interface::*;
pub use node_id::*;
pub use path::cycle_remover::*;
pub use path::in_order_cycle_remover::*;
pub use path::shortest_first_path_simplifier::*;
pub use path::simplifier::*;
pub use path::*;
pub use path_id::*;
pub use physical_neighbor_table::*;
pub use routing_table::flat_routing_table::*;
pub use routing_table::*;
pub use state_seq_nr::*;

pub mod bucket;
pub mod contact;
pub mod insertion_strategy;
pub mod interface;
pub mod node_id;
pub mod path;
pub mod path_id;
pub mod physical_neighbor_table;
pub mod routing_table;
pub mod state_seq_nr;

/// A physical connection between two nodes.
#[derive(Debug, Eq, PartialEq, Hash, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct Link(NodeId, NodeId);

impl Link {
    pub fn new(first: NodeId, second: NodeId) -> Self {
        if first < second {
            Link(first, second)
        } else {
            Link(second, first)
        }
    }

    pub fn first(&self) -> &NodeId {
        &self.0
    }

    pub fn second(&self) -> &NodeId {
        &self.1
    }
}

impl From<(NodeId, NodeId)> for Link {
    fn from((first, second): (NodeId, NodeId)) -> Self {
        Link::new(first, second)
    }
}

/// Data structure representing nodes or physical connections to not use while forwarding protocol
/// messages.
///
/// Not via data is supposed to only represent information in the routing table.
/// It's an error for the local not via data to contain entries not affecting any nodes in the
/// routing table.
/// When a node gets deleted, all the not via data mentioning it will be removed.
#[derive(Debug, PartialEq, Eq, Clone, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum NotVia {
    Link(Link),
}

impl Display for NotVia {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Link(Link(left, right)) => {
                write!(f, "NotVia (")?;
                Display::fmt(left, f)?;
                write!(f, ", ")?;
                Display::fmt(right, f)?;
                write!(f, ")")
            }
        }
    }
}
