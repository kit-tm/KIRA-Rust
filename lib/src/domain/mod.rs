// To change default NodeId simply change this
pub use contact::*;
pub use node_id::*;
pub use path::*;

pub mod contact;
pub mod node_id;
pub mod path;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Link(NodeId, NodeId);
