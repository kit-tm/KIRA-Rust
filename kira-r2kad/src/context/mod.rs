//! [UseCaseContext] related structures and traits.

use std::cell::{Ref, RefMut};
use std::collections::HashSet;

pub use sync_context::SyncContext;

use crate::domain::{NodeId, NotViaState};

pub mod sync_context;

/// Configuration Wrapper for all dependencies of a [UseCaseContext].
pub struct ContextConfig<RT, RU, IS, UN, VG> {
    pub root_id: NodeId,
    pub routing_table: RT,
    pub runtime: RU,
    pub insertion_strategy: IS,
    pub uln_table: UN,
    pub not_via_state: HashSet<NotViaState>,
    pub vicinity_graph: VG,
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
    type UnderlayNeighborTable: Sized;
    type VicinityGraph: Sized;

    #[allow(clippy::type_complexity)]
    fn new(
        config: ContextConfig<
            Self::RoutingTable,
            Self::Runtime,
            Self::InsertionStrategy,
            Self::UnderlayNeighborTable,
            Self::VicinityGraph,
        >,
    ) -> Self;

    fn root_id(&self) -> &NodeId;

    fn routing_table(&self) -> Ref<'_, Self::RoutingTable>;

    fn routing_table_mut(&self) -> RefMut<'_, Self::RoutingTable>;

    fn routing_table_insertion_strategy(&self) -> RefMut<'_, Self::InsertionStrategy>;

    fn uln_table(&self) -> Ref<'_, Self::UnderlayNeighborTable>;

    fn uln_table_mut(&self) -> RefMut<'_, Self::UnderlayNeighborTable>;

    fn runtime(&self) -> &Self::Runtime;

    fn not_via_state(&self) -> Ref<'_, HashSet<NotViaState>>;

    fn not_via_state_mut(&self) -> RefMut<'_, HashSet<NotViaState>>;

    fn vicinity_graph(&self) -> Ref<'_, Self::VicinityGraph>;

    fn vicinity_graph_mut(&self) -> RefMut<'_, Self::VicinityGraph>;
}
