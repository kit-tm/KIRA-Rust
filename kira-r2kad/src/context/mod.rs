//! [UseCaseContext] related structures and traits.

use std::cell::{Ref, RefMut};
use std::collections::HashSet;

pub use sync_context::SyncContext;

use crate::domain::{NodeId, NotVia};

pub mod sync_context;

/// Configuration Wrapper for all dependencies of a [UseCaseContext].
pub struct ContextConfig<RT, RU, IS, PN> {
    pub root_id: NodeId,
    pub routing_table: RT,
    pub runtime: RU,
    pub insertion_strategy: IS,
    pub pn_table: PN,
    pub not_via: HashSet<NotVia>,
}

/// Context a UseCase runs in.
///
/// Provides access to the shared global state of the node.
///
/// NotVia Data represents links and nodes in the routing table which are not longer functional.
///
/// NotVia Data of other nodes will only be added if they affect contacts in the own routing table.
pub trait UseCaseContext {
    type RoutingTable: Sized;
    type InsertionStrategy: Sized;
    type Runtime: Sized;
    type PhysicalNeighborTable: Sized;

    fn new(
        config: ContextConfig<
            Self::RoutingTable,
            Self::Runtime,
            Self::InsertionStrategy,
            Self::PhysicalNeighborTable,
        >,
    ) -> Self;

    fn root_id(&self) -> &NodeId;

    fn routing_table(&self) -> Ref<'_, Self::RoutingTable>;

    fn routing_table_mut(&self) -> RefMut<'_, Self::RoutingTable>;

    fn routing_table_insertion_strategy(&self) -> RefMut<'_, Self::InsertionStrategy>;

    fn pn_table(&self) -> Ref<'_, Self::PhysicalNeighborTable>;

    fn pn_table_mut(&self) -> RefMut<'_, Self::PhysicalNeighborTable>;

    fn runtime(&self) -> &Self::Runtime;

    fn not_via(&self) -> Ref<'_, HashSet<NotVia>>;

    fn not_via_mut(&self) -> RefMut<'_, HashSet<NotVia>>;
}
