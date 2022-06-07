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

#[derive(Debug, Eq, PartialEq)]
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

#[derive(Debug, Eq, PartialEq)]
pub struct UnsplittableBucket;

impl Display for UnsplittableBucket {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Bucket can't be split")
    }
}

impl Error for UnsplittableBucket {}

pub trait RoutingTable<'a, const ID_SIZE: usize> {
    type Iter: Iterator<Item = &'a Contact<ID_SIZE>>;

    /// Add a new [Contact] to the [RoutingTable].
    ///
    /// Returns an Error if the [Bucket] for the [Contact] is full or
    /// a [Contact] with the same [NodeId] is already present in the [Bucket].
    ///
    /// This doesn't perform any decision making if a bucket has to be split or another
    /// contact has to be replaced.
    /// This is entirely up to the caller.
    fn add(&'a mut self, contact: Contact<ID_SIZE>) -> Result<(), AddError<ID_SIZE>>;

    /// Removes an existing [Contact] and returns it if present.
    fn remove(&'a mut self, id: &NodeId<ID_SIZE>) -> Option<Contact<ID_SIZE>>;

    /// Returns an existing [Contact] if present.
    fn contact(&'a self, id: &NodeId<ID_SIZE>) -> Option<&Contact<ID_SIZE>>;

    /// Returns a random [Contact] if the [RoutingTable] is not empty.
    fn random_contact(&'a self) -> Option<&Contact<ID_SIZE>>;

    /// Returns a mutable reference to an existing [Contact] if present.
    fn contact_mut(&'a mut self, id: &NodeId<ID_SIZE>) -> Option<&mut Contact<ID_SIZE>>;

    /// Returns if a [Contact] with a given [NodeId] is present in the [RoutingTable].
    fn contains(&'a self, id: &NodeId<ID_SIZE>) -> bool;

    /// Returns an [Iterator] over all [Contact]s in this [RoutingTable].
    fn contacts_iter(&'a self) -> Self::Iter;

    /// Attempts to split the [Bucket] the id should be located in.
    /// The [Contact]s in the [Bucket] will be inserted in the appropriate [Bucket]s.
    ///
    /// This doesn't require a [Contact] inside the [Bucket] with the id.
    fn split_bucket(&'a mut self, id: &NodeId<ID_SIZE>) -> Result<(), UnsplittableBucket>;
}
