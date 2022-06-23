// To change default NodeId simply change this
pub use bucket::*;
pub use contact::*;
pub use discovery_table::*;
pub use insertion_strategy::*;
pub use neighbor_table::*;
pub use node_id::*;
pub use path::*;
pub use path_simplifier::*;
pub use path_validator::*;
pub use routing_table::flat_routing_table::*;
pub use routing_table::*;

mod bucket;
mod contact;
mod discovery_table;
mod insertion_strategy;
mod neighbor_table;
mod node_id;
mod path;
mod path_simplifier;
mod path_validator;
mod routing_table;

pub type Link<const ID_SIZE: usize> = (NodeId<ID_SIZE>, NodeId<ID_SIZE>);
