// To change default NodeId simply change this
pub use bucket::*;
pub use contact::*;
pub use flat_routing_table::*;
pub use insertion_strategy::*;
pub use node_id::*;
pub use path::*;
pub use routing_table::*;

mod bucket;
mod contact;
mod flat_routing_table;
mod insertion_strategy;
mod node_id;
mod path;
mod routing_table;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Link<const ID_SIZE: usize>(NodeId<ID_SIZE>, NodeId<ID_SIZE>);
