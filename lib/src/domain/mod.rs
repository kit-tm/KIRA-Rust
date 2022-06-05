// To change default NodeId simply change this
pub use bucket::*;
pub use contact::*;
pub use node_id::*;
pub use path::*;

pub mod bucket;
pub mod contact;
pub mod node_id;
pub mod path;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Link<const ID_SIZE: usize = DEFAULT_ID_SIZE>(NodeId<ID_SIZE>, NodeId<ID_SIZE>);
