use std::error::Error;
use std::fmt::{Display, Formatter};

// To change default NodeId simply change this
pub use bucket::*;
pub use contact::*;
pub use flat_routing_table::*;
pub use node_id::*;
pub use path::*;

mod bucket;
mod contact;
mod flat_routing_table;
mod node_id;
mod path;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Link<const ID_SIZE: usize>(NodeId<ID_SIZE>, NodeId<ID_SIZE>);

#[derive(Debug)]
pub enum AddError<const ID_SIZE: usize> {
    AlreadyExists(NodeId<ID_SIZE>),
    NotAdded,
}

impl<const ID_SIZE: usize> Display for AddError<ID_SIZE> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAdded => write!(f, "Bucket is full, not added"),
            Self::AlreadyExists(id) => write!(f, "Contact with id {} already exists", id),
        }
    }
}

impl<const ID_SIZE: usize> Error for AddError<ID_SIZE> {}

pub trait RoutingTable<'a, const ID_SIZE: usize> {
    type Iter: Iterator<Item = &'a Contact<ID_SIZE>>;

    /// Add a new [Contact] to the [RoutingTable].
    fn add(&'a mut self, contact: Contact<ID_SIZE>) -> Result<(), AddError<ID_SIZE>>;

    /// Removes an existing [Contact] and returns it if present.
    fn remove(&'a mut self, id: &NodeId<ID_SIZE>) -> Option<Contact<ID_SIZE>>;

    /// Returns an existing [Contact] if present.
    fn get(&'a self, id: &NodeId<ID_SIZE>) -> Option<&Contact<ID_SIZE>>;

    /// Returns a random [Contact] if the [RoutingTable] is not empty.
    fn get_random(&'a self) -> Option<&Contact<ID_SIZE>>;

    /// Returns a mutable reference to an existing [Contact] if present.
    fn get_mut(&'a mut self, id: &NodeId<ID_SIZE>) -> Option<&mut Contact<ID_SIZE>>;

    /// Returns if a [Contact] with a given [NodeId] is present in the [RoutingTable].
    fn contains(&'a self, id: &NodeId<ID_SIZE>) -> bool;

    /// Returns an [Iterator] over all [Contact]s in this [RoutingTable].
    fn contacts_iter(&'a self) -> Self::Iter;
}
