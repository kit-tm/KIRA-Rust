// To change default NodeId simply change this
pub use bucket::*;
pub use contact::*;
pub use insertion_strategy::*;
pub use neighbor_table::*;
pub use node_id::*;
pub use path::*;
pub use path_simplifier::*;
pub use path_validator::*;
pub use routing_table::flat_routing_table::*;
pub use routing_table::*;

pub mod bucket;
pub mod contact;
pub mod insertion_strategy;
pub mod neighbor_table;
pub mod node_id;
pub mod path;
pub mod path_simplifier;
pub mod path_validator;
pub mod routing_table;

pub type Link = (NodeId, NodeId);
