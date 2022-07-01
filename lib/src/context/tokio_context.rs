use std::ops::Deref;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::broadcaster::Broadcaster;
use crate::context::{ReadGuard, UseCaseContext, WriteGuard};
use crate::domain::{NeighborTable, NodeId};
use crate::runtime::TokioRuntime;
use crate::utils::tokio_utils;

/// Runtime for using a tokio runtime.
///
/// # Limitiations
///
/// Calling the [Context] methods inside a async environment is currently only supported
/// in a rt-multi-thread Tokio runtime.
#[derive(Debug, Clone)]
pub struct TokioContext<RT, MS, RU, IS> {
    root_id: NodeId,
    routing_table: Arc<RwLock<RT>>,
    neighbor_table: Arc<RwLock<NeighborTable>>,
    insertion_strategy: Arc<RwLock<IS>>,
    message_sender: Arc<RwLock<MS>>,
    runtime: RU,
}

impl<RT, MS, B: Broadcaster, IS> TokioContext<RT, MS, TokioRuntime<B>, IS> {
    /// Creates a new [Context].
    pub fn new(
        root_id: NodeId,
        routing_table: RT,
        neighbor_table: NeighborTable,
        insertion_strategy: IS,
        message_sender: MS,
        runtime: TokioRuntime<B>,
    ) -> Self {
        Self {
            root_id,
            routing_table: Arc::new(RwLock::new(routing_table)),
            neighbor_table: Arc::new(RwLock::new(neighbor_table)),
            insertion_strategy: Arc::new(RwLock::new(insertion_strategy)),
            message_sender: Arc::new(RwLock::new(message_sender)),
            runtime,
        }
    }
}

impl<RT, MS, B: Broadcaster, IS> UseCaseContext for TokioContext<RT, MS, TokioRuntime<B>, IS> {
    type RoutingTable = RT;
    type MessageSender = MS;
    type Runtime = TokioRuntime<B>;
    type InsertionStrategy = IS;

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

    fn neighbor_table(&self) -> ReadGuard<NeighborTable> {
        tokio_utils::get_read_guard(self.neighbor_table.deref()).into()
    }

    fn neighbor_table_mut(&self) -> WriteGuard<NeighborTable> {
        tokio_utils::get_write_guard(self.neighbor_table.deref()).into()
    }

    fn message_sender(&self) -> ReadGuard<MS> {
        tokio_utils::get_read_guard(self.message_sender.deref()).into()
    }

    fn message_sender_mut(&self) -> WriteGuard<MS> {
        tokio_utils::get_write_guard(self.message_sender.deref()).into()
    }

    fn runtime(&self) -> &TokioRuntime<B> {
        &self.runtime
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::runtime;
    use tokio::sync::broadcast;

    use crate::context::{ReadGuard, TokioContext, UseCaseContext, WriteGuard};
    use crate::domain::{
        FlatRoutingTable, InsertionStrategyResult, NeighborTable, NodeId, TestInsertionStrategy,
    };
    use crate::messaging::InMemoryMessageHub;
    use crate::runtime::TokioRuntime;

    #[test]
    fn sync_context_execution() {
        let runtime = runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("failed to create runtime");

        let root_id = NodeId::zero();

        let message_hub = InMemoryMessageHub::new();

        let (broadcaster, _) = broadcast::channel(1);

        let runtime = TokioRuntime::new(broadcaster, Arc::new(runtime));

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let context = TokioContext::new(
            root_id.clone(),
            FlatRoutingTable::<20, 1>::new(root_id),
            NeighborTable::new(),
            insertion_strategy,
            message_hub,
            runtime,
        );

        // Check for getters to not panic
        assert!(matches!(context.routing_table(), ReadGuard::Async(_)));
        assert!(matches!(context.neighbor_table(), ReadGuard::Async(_)));
        assert!(matches!(context.message_sender(), ReadGuard::Async(_)));

        assert!(matches!(context.routing_table_mut(), WriteGuard::Async(_)));
        assert!(matches!(context.neighbor_table_mut(), WriteGuard::Async(_)));
        assert!(matches!(context.message_sender_mut(), WriteGuard::Async(_)));
    }
}
