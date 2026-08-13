use std::cell::{
    Ref,
    RefCell,
    RefMut,
};

use crate::{
    context::ContextConfig,
    domain::NodeId,
    use_cases::UseCaseContext,
};

/// Implements a [UseCaseContext] which can only be used in a single threaded synchronous environment.
#[derive(Debug)]
pub struct SyncContext<RT, RU, IS, UN, VG> {
    root_id: NodeId,
    routing_table: RefCell<RT>,
    insertion_strategy: RefCell<IS>,
    un_table: RefCell<UN>,
    runtime: RU,
    vicinity_graph: RefCell<VG>,
}

impl<RT, RU, IS, UN, VG> UseCaseContext for SyncContext<RT, RU, IS, UN, VG> {
    type InsertionStrategy = IS;
    type RoutingTable = RT;
    type Runtime = RU;
    type UnderlayNeighborTable = UN;
    type VicinityGraph = VG;

    fn new(config: ContextConfig<RT, RU, IS, UN, VG>) -> Self {
        Self {
            root_id: config.root_id,
            routing_table: RefCell::new(config.routing_table),
            insertion_strategy: RefCell::new(config.insertion_strategy),
            un_table: RefCell::new(config.uln_table),
            runtime: config.runtime,
            vicinity_graph: RefCell::new(config.vicinity_graph),
        }
    }

    fn root_id(&self) -> &NodeId {
        &self.root_id
    }

    fn routing_table(&self) -> Ref<'_, RT> {
        self.routing_table.borrow()
    }

    fn routing_table_mut(&self) -> RefMut<'_, RT> {
        self.routing_table.borrow_mut()
    }

    fn routing_table_insertion_strategy(&self) -> RefMut<'_, IS> {
        self.insertion_strategy.borrow_mut()
    }

    fn uln_table(&self) -> Ref<'_, Self::UnderlayNeighborTable> {
        self.un_table.borrow()
    }

    fn uln_table_mut(&self) -> RefMut<'_, Self::UnderlayNeighborTable> {
        self.un_table.borrow_mut()
    }

    fn runtime(&self) -> &Self::Runtime {
        &self.runtime
    }

    fn vicinity_graph(&self) -> Ref<'_, Self::VicinityGraph> {
        self.vicinity_graph.borrow()
    }

    fn vicinity_graph_mut(&self) -> RefMut<'_, Self::VicinityGraph> {
        self.vicinity_graph.borrow_mut()
    }
}
