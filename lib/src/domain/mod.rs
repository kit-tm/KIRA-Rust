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

pub trait RoutingTable<'a, const ID_SIZE: usize> {
    type ClosestIter: Iterator<Item = &'a Contact<ID_SIZE>>;
    type Iter: Iterator<Item = &'a Contact<ID_SIZE>>;

    /// Update a contact in the [RoutingTable] by either updating an existing Entry
    /// or adding a new one.
    fn update(&mut self, contact: Contact<ID_SIZE>);

    /// Removes an existing [Contact] and returns it if present.
    fn remove(&mut self, id: &NodeId<ID_SIZE>) -> Option<Contact<ID_SIZE>>;

    /// Returns an existing [Contact] if present.
    fn get(&self, id: &NodeId<ID_SIZE>) -> Option<&Contact<ID_SIZE>>;

    /// Returns a random [Contact] if the [RoutingTable] is not empty.
    fn get_random(&self) -> Option<&Contact<ID_SIZE>>;

    /// Returns a mutable reference to an existing [Contact] if present.
    fn get_mut(&mut self, id: &NodeId<ID_SIZE>) -> Option<&mut Contact<ID_SIZE>>;

    /// Returns an [Iterator] over the closest [Contact]s of a [NodeId].
    fn get_closest_iter(&self, id: &NodeId<ID_SIZE>) -> Self::ClosestIter;

    /// Returns if a [Contact] with a given [NodeId] is present in the [RoutingTable].
    fn contains(&self, id: &NodeId<ID_SIZE>) -> bool;

    /// Returns an [Iterator] over all [Contact]s in this [RoutingTable].
    fn contacts_iter(&self) -> Self::Iter;
}
