use std::cell::{Ref, RefCell, RefMut};
use std::collections::HashSet;

use crate::domain::{NodeId, NotVia};
use crate::use_cases::context::ContextConfig;
use crate::use_cases::{UseCaseContext, UseCaseRuntime};

/// Implements a [UseCaseContext] which can only be used in a single threaded synchronous environment.
///
/// In comparison to implementations like [TokioContext](crate::context::tokio_context::TokioContext) this
/// implementation is free of locks and their runtime overhead due to the assumption that
/// everything happens in a single thread.
#[derive(Debug)]
pub struct SyncContext<RT, IS, PN> {
    root_id: NodeId,
    routing_table: RefCell<RT>,
    insertion_strategy: RefCell<IS>,
    pn_table: RefCell<PN>,
    runtime: RefCell<UseCaseRuntime>,
    not_via: RefCell<HashSet<NotVia>>,
}

impl<RT, IS, PN> UseCaseContext for SyncContext<RT, IS, PN> {
    type RoutingTable = RT;
    type InsertionStrategy = IS;
    type PhysicalNeighborTable = PN;

    fn new(config: ContextConfig<RT, IS, PN>) -> Self {
        Self {
            root_id: config.root_id,
            routing_table: RefCell::new(config.routing_table),
            insertion_strategy: RefCell::new(config.insertion_strategy),
            pn_table: RefCell::new(config.pn_table),
            runtime: config.runtime,
            not_via: RefCell::new(config.not_via),
        }
    }

    fn root_id(&self) -> &NodeId {
        &self.root_id
    }

    fn routing_table(&self) -> Ref<RT> {
        self.routing_table.borrow()
    }

    fn routing_table_mut(&self) -> RefMut<RT> {
        self.routing_table.borrow_mut()
    }

    fn routing_table_insertion_strategy(&self) -> RefMut<IS> {
        self.insertion_strategy.borrow_mut()
    }

    fn pn_table(&self) -> Ref<'_, Self::PhysicalNeighborTable> {
        self.pn_table.borrow()
    }

    fn pn_table_mut(&self) -> RefMut<'_, Self::PhysicalNeighborTable> {
        self.pn_table.borrow_mut()
    }

    fn runtime(&self) -> Ref<'_, UseCaseRuntime> {
        self.runtime.borrow()
    }

    fn runtime_mut(&self) -> RefMut<'_, UseCaseRuntime> {
        self.runtime.borrow_mut()
    }

    fn not_via(&self) -> Ref<'_, HashSet<NotVia>> {
        self.not_via.borrow()
    }

    fn not_via_mut(&self) -> RefMut<'_, HashSet<NotVia>> {
        self.not_via.borrow_mut()
    }
}
