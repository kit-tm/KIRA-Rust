use std::fmt::{Debug, Display};
use std::ops::BitXor;

// To change default NodeId simply change this
pub use const_node_id::ConstNodeId as NodeId;
pub use path::Path;

pub const DEFAULT_ID_SIZE: usize = 14;

pub mod const_node_id;
mod path;

/// Components implementing this trait represent an Node-Id.
///
/// Separated from concrete Node-Id implementations to make them replaceable.
pub trait Id: Debug + BitXor + Sized + Eq + Clone + Display {
    /// Returns the shared prefix length in number of bits
    fn shared_prefix_bits(&self, other: &Self) -> usize {
        self.shared_prefix_len(other, 1)
    }

    /// Returns the shared prefix length in number of groups of bits.
    fn shared_prefix_len(&self, other: &Self, bits_per_group: usize) -> usize;
}
