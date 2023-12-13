use std::collections::HashSet;
use std::ops::Deref;
use std::sync::Arc;
use pnet::packet::ipv6::Routing;

use tokio::sync::RwLock;

use crate::broadcaster::Broadcaster;
use crate::context::{ContextConfig, ReadGuard, UseCaseContext, WriteGuard};
use crate::domain::{DiscoveryRangeProvider, NodeId, NotVia, PNTable, RoutingTable};
use crate::domain::api::{DiscoveryRange};
use crate::runtime::TokioRuntime;
use crate::utils::tokio_utils;

/// Runtime for using a tokio runtime.
///
/// # Limitiations
///
/// Calling the [UseCaseContext] methods is currently only supported outside of an async runtime
/// or the rt-multi-thread Tokio runtime.
#[derive(Debug, Clone)]
pub struct TokioContext<RT, MS, RU, IS, FT> {
    root_id: NodeId,
    routing_table: Arc<RwLock<RT>>,
    pn_table: Arc<RwLock<PNTable>>,
    insertion_strategy: Arc<RwLock<IS>>,
    message_sender: Arc<RwLock<MS>>,
    forwarding_tables: Arc<RwLock<FT>>,
    runtime: RU,
    not_via: Arc<RwLock<HashSet<NotVia>>>,
}

impl<RT, MS, B: Broadcaster, IS, FT> UseCaseContext
    for TokioContext<RT, MS, TokioRuntime<B>, IS, FT>
where
    for<'a> RT: Into<crate::domain::api::RoutingTable> + DiscoveryRangeProvider + Clone
{
    type RoutingTable = RT;
    type MessageSender = MS;
    type Runtime = TokioRuntime<B>;
    type InsertionStrategy = IS;
    type ForwardingTables = FT;

    fn new(config: ContextConfig<RT, MS, TokioRuntime<B>, IS, FT>) -> Self {
        Self {
            root_id: config.root_id,
            routing_table: Arc::new(RwLock::new(config.routing_table)),
            pn_table: Arc::new(RwLock::new(config.pn_table)),
            insertion_strategy: Arc::new(RwLock::new(config.insertion_strategy)),
            message_sender: Arc::new(RwLock::new(config.message_sender)),
            forwarding_tables: Arc::new(RwLock::new(config.forwarding_tables)),
            runtime: config.runtime,
            not_via: Arc::new(RwLock::new(config.not_via)),
        }
    }

    fn root_id(&self) -> &NodeId {
        &self.root_id
    }

    fn routing_table(&self) -> ReadGuard<RT> {
        tokio_utils::get_read_guard(self.routing_table.deref()).into()
    }

    fn routing_table_mut(&self) -> WriteGuard<RT> {
        tokio_utils::get_write_guard(self.routing_table.deref()).into()
    }

    fn routing_table_insertion_strategy(&self) -> WriteGuard<Self::InsertionStrategy> {
        tokio_utils::get_write_guard(self.insertion_strategy.deref()).into()
    }

    fn pn_table(&self) -> ReadGuard<PNTable> {
        tokio_utils::get_read_guard(self.pn_table.deref()).into()
    }

    fn pn_table_mut(&self) -> WriteGuard<PNTable> {
        tokio_utils::get_write_guard(self.pn_table.deref()).into()
    }

    fn message_sender(&self) -> ReadGuard<MS> {
        tokio_utils::get_read_guard(self.message_sender.deref()).into()
    }

    fn message_sender_mut(&self) -> WriteGuard<MS> {
        tokio_utils::get_write_guard(self.message_sender.deref()).into()
    }

    fn forwarding_tables(&self) -> ReadGuard<'_, Self::ForwardingTables> {
        tokio_utils::get_read_guard(self.forwarding_tables.deref()).into()
    }

    fn forwarding_tables_mut(&self) -> WriteGuard<'_, Self::ForwardingTables> {
        tokio_utils::get_write_guard(self.forwarding_tables.deref()).into()
    }

    fn runtime(&self) -> &TokioRuntime<B> {
        &self.runtime
    }

    fn not_via(&self) -> ReadGuard<'_, HashSet<NotVia>> {
        tokio_utils::get_read_guard(self.not_via.deref()).into()
    }

    fn not_via_mut(&self) -> WriteGuard<'_, HashSet<NotVia>> {
        tokio_utils::get_write_guard(self.not_via.deref()).into()
    }

    fn to_api_model(&self) -> (crate::domain::api::RoutingTable, DiscoveryRange) {
        let read_guard: ReadGuard<Self::RoutingTable> = tokio_utils::get_read_guard(self.routing_table.deref()).into();
        let rt_old: Self::RoutingTable = (*read_guard.deref()).clone();
        let discovery_range = rt_old.get_discovery_range();

        (rt_old.into(), discovery_range)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Arc;

    use tokio::runtime;
    use tokio::sync::broadcast;

    use crate::context::{ContextConfig, ReadGuard, TokioContext, UseCaseContext, WriteGuard};
    use crate::domain::{
        FlatRoutingTable, InsertionStrategyResult, NodeId, PNTable, TestInsertionStrategy,
    };
    use crate::forwarding::in_memory_tables::InMemoryFwdTables;
    use crate::messaging::InMemoryMessageChannel;
    use crate::runtime::TokioRuntime;

    #[test]
    fn sync_context_execution() {
        let runtime = runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("failed to create runtime");

        let root_id = NodeId::zero();

        let message_hub = InMemoryMessageChannel::default();

        let (broadcaster, _) = broadcast::channel(1);

        let runtime = TokioRuntime::new(broadcaster, Arc::new(runtime));

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let forwarding_tables = InMemoryFwdTables::new();

        let context = TokioContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table: FlatRoutingTable::<20, 1>::new(root_id),
            pn_table: PNTable::new(),
            insertion_strategy,
            message_sender: message_hub,
            runtime,
            forwarding_tables,
            not_via: HashSet::default(),
        });

        // Check for getters to not panic
        assert!(matches!(context.routing_table(), ReadGuard::Async(_)));
        assert!(matches!(context.pn_table(), ReadGuard::Async(_)));
        assert!(matches!(context.message_sender(), ReadGuard::Async(_)));

        assert!(matches!(context.routing_table_mut(), WriteGuard::Async(_)));
        assert!(matches!(context.pn_table_mut(), WriteGuard::Async(_)));
        assert!(matches!(context.message_sender_mut(), WriteGuard::Async(_)));
    }
}
