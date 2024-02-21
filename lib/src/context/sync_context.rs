use std::cell::RefCell;
use std::collections::HashSet;
use tracing::instrument;

use crate::context::{ContextConfig, ReadGuard, UseCaseContext, WriteGuard};
use crate::domain::{DiscoveryRangeProvider, NodeId, NotVia, PNTable};
use crate::domain::api::{DiscoveryRange, NodeIdApi};

/// Implements a [UseCaseContext] which can only be used in a single threaded synchronous environment.
///
/// In comparison to implementations like [TokioContext](crate::context::tokio_context::TokioContext) this
/// implementation is free of locks and their runtime overhead due to the assumption that
/// everything happens in a single thread.
#[derive(Debug)]
pub struct SyncContext<RT, MS, RU, IS, FT> {
    root_id: NodeId,
    routing_table: RefCell<RT>,
    insertion_strategy: RefCell<IS>,
    pn_table: RefCell<PNTable>,
    message_sender: RefCell<MS>,
    runtime: RU,
    forwarding_tables: RefCell<FT>,
    not_via: RefCell<HashSet<NotVia>>,
}

impl<RT, MS, RU, IS, FT> UseCaseContext for SyncContext<RT, MS, RU, IS, FT>
where for<'a> RT: Into<crate::domain::api::RoutingTable> + DiscoveryRangeProvider + Clone{
    type RoutingTable = RT;
    type MessageSender = MS;
    type Runtime = RU;
    type InsertionStrategy = IS;
    type ForwardingTables = FT;

    fn new(config: ContextConfig<RT, MS, RU, IS, FT>) -> Self {
        Self {
            root_id: config.root_id,
            routing_table: RefCell::new(config.routing_table),
            insertion_strategy: RefCell::new(config.insertion_strategy),
            pn_table: RefCell::new(config.pn_table),
            message_sender: RefCell::new(config.message_sender),
            runtime: config.runtime,
            forwarding_tables: RefCell::new(config.forwarding_tables),
            not_via: RefCell::new(config.not_via),
        }
    }

    fn root_id(&self) -> &NodeId {
        &self.root_id
    }

    fn routing_table(&self) -> ReadGuard<RT> {
        self.routing_table.borrow().into()
    }

    fn routing_table_mut(&self) -> WriteGuard<RT> {
        self.routing_table.borrow_mut().into()
    }

    fn routing_table_insertion_strategy(&self) -> WriteGuard<IS> {
        self.insertion_strategy.borrow_mut().into()
    }

    fn pn_table(&self) -> ReadGuard<PNTable> {
        self.pn_table.borrow().into()
    }

    fn pn_table_mut(&self) -> WriteGuard<PNTable> {
        self.pn_table.borrow_mut().into()
    }

    fn message_sender(&self) -> ReadGuard<MS> {
        self.message_sender.borrow().into()
    }

    fn message_sender_mut(&self) -> WriteGuard<MS> {
        self.message_sender.borrow_mut().into()
    }

    fn forwarding_tables(&self) -> ReadGuard<'_, Self::ForwardingTables> {
        self.forwarding_tables.borrow().into()
    }

    fn forwarding_tables_mut(&self) -> WriteGuard<'_, Self::ForwardingTables> {
        self.forwarding_tables.borrow_mut().into()
    }

    fn runtime(&self) -> &RU {
        &self.runtime
    }

    fn not_via(&self) -> ReadGuard<'_, HashSet<NotVia>> {
        self.not_via.borrow().into()
    }

    fn not_via_mut(&self) -> WriteGuard<'_, HashSet<NotVia>> {
        self.not_via.borrow_mut().into()
    }

    #[tracing::instrument(level = "warn", name="copy routing table", skip(self), fields(node_id))]
    fn to_api_model(&self) -> (crate::domain::api::RoutingTable, DiscoveryRange) {
        let node_id: NodeIdApi = self.root_id.clone().into();
        tracing::Span::current().record("node_id", node_id.node_id);
        let rt: Self::RoutingTable = self.routing_table.borrow().clone();
        let discovery_range = rt.get_discovery_range();
        (rt.into(), discovery_range)
    }
}
