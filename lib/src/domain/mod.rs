use std::fmt::{Display, Formatter};
use std::ops::{Add, AddAssign};

// To change default NodeId simply change this
pub use bucket::*;
pub use contact::*;
pub use insertion_strategy::*;
pub use neighbor_table::*;
pub use node_id::*;
pub use path::*;
pub use path_simplifier::*;
pub use routing_table::flat_routing_table::*;
pub use routing_table::*;

pub mod bucket;
pub mod contact;
pub mod insertion_strategy;
pub mod neighbor_table;
pub mod node_id;
pub mod path;
pub mod path_simplifier;
pub mod routing_table;

pub type Link = (NodeId, NodeId);

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct StateSeqNr(u64);

impl From<u64> for StateSeqNr {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl Add<u64> for StateSeqNr {
    type Output = StateSeqNr;

    fn add(self, rhs: u64) -> Self::Output {
        Self(self.0 + rhs)
    }
}

impl AddAssign<u64> for StateSeqNr {
    fn add_assign(&mut self, rhs: u64) {
        self.0 += rhs;
    }
}

impl Display for StateSeqNr {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
