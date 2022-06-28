use std::sync::Arc;

use tokio::sync::RwLock;

use crate::broadcaster::Broadcaster;
use crate::context::{Context, ReadGuard, WriteGuard};
use crate::domain::NodeId;
use crate::runtime::TokioRuntime;

/// Runtime for using a tokio runtime.
///
/// # Limitiations
///
/// Calling the [Context] methods inside a async context is currently not supported
/// due to the sync architecture of the UseCases.
#[derive(Debug, Clone)]
pub struct TokioContext<RT, NT, MS, RU> {
    root_id: NodeId,
    routing_table: Arc<RwLock<RT>>,
    neighbor_table: Arc<RwLock<NT>>,
    message_sender: Arc<RwLock<MS>>,
    runtime: RU,
}

impl<RT, NT, MS, B: Broadcaster> TokioContext<RT, NT, MS, TokioRuntime<B>> {
    /// Creates a new [Context].
    pub fn new(
        root_id: NodeId,
        routing_table: RT,
        neighbor_table: NT,
        message_sender: MS,
        runtime: TokioRuntime<B>,
    ) -> Self {
        Self {
            root_id,
            routing_table: Arc::new(RwLock::new(routing_table)),
            neighbor_table: Arc::new(RwLock::new(neighbor_table)),
            message_sender: Arc::new(RwLock::new(message_sender)),
            runtime,
        }
    }
}

impl<RT, NT, MS, B: Broadcaster> Context for TokioContext<RT, NT, MS, TokioRuntime<B>> {
    type RoutingTable = RT;
    type NeighborTable = NT;
    type MessageSender = MS;
    type Runtime = TokioRuntime<B>;

    fn root_id(&self) -> &NodeId {
        &self.root_id
    }

    fn routing_table(&self) -> ReadGuard<RT> {
        self.routing_table.blocking_read().into()
    }

    fn routing_table_mut(&self) -> WriteGuard<RT> {
        self.routing_table.blocking_write().into()
    }

    fn neighbor_table(&self) -> ReadGuard<NT> {
        self.neighbor_table.blocking_read().into()
    }

    fn neighbor_table_mut(&self) -> WriteGuard<NT> {
        self.neighbor_table.blocking_write().into()
    }

    fn message_sender(&self) -> ReadGuard<MS> {
        self.message_sender.blocking_read().into()
    }

    fn message_sender_mut(&self) -> WriteGuard<MS> {
        self.message_sender.blocking_write().into()
    }

    fn runtime(&self) -> &TokioRuntime<B> {
        &self.runtime
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use tokio::runtime;
    use tokio::sync::broadcast;

    use crate::context::{Context, ReadGuard, TokioContext, WriteGuard};
    use crate::domain::{FlatRoutingTable, NodeId, Port};
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

        let context = TokioContext::new(
            root_id.clone(),
            FlatRoutingTable::<20, 1>::new(root_id),
            HashMap::<NodeId, Port>::new(),
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
