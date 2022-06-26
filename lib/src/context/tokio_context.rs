use std::sync::Arc;

use tokio::sync::RwLock;

use crate::broadcaster::Broadcaster;
use crate::context::{Context, ReadGuard, WriteGuard};
use crate::domain::NodeId;
use crate::runtime::TokioRuntime;

#[derive(Debug, Clone)]
pub struct TokioContext<RT, NT, DT, MS, RU> {
    root_id: NodeId,
    routing_table: Arc<RwLock<RT>>,
    neighbor_table: Arc<RwLock<NT>>,
    discovery_table: Arc<RwLock<DT>>,
    message_sender: Arc<RwLock<MS>>,
    runtime: RU,
}

impl<RT, NT, DT, MS, B: Broadcaster> TokioContext<RT, NT, DT, MS, TokioRuntime<B>> {
    /// Creates a new [Context].
    pub fn new(
        root_id: NodeId,
        routing_table: RT,
        neighbor_table: NT,
        discovery_table: DT,
        message_sender: MS,
        runtime: TokioRuntime<B>,
    ) -> Self {
        Self {
            root_id,
            routing_table: Arc::new(RwLock::new(routing_table)),
            neighbor_table: Arc::new(RwLock::new(neighbor_table)),
            discovery_table: Arc::new(RwLock::new(discovery_table)),
            message_sender: Arc::new(RwLock::new(message_sender)),
            runtime,
        }
    }
}

impl<RT, NT, DT, MS, B: Broadcaster> Context for TokioContext<RT, NT, DT, MS, TokioRuntime<B>> {
    type RoutingTable = RT;
    type NeighborTable = NT;
    type DiscoveryTable = DT;
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

    fn discovery_table(&self) -> ReadGuard<DT> {
        self.discovery_table.blocking_read().into()
    }

    fn discovery_table_mut(&self) -> WriteGuard<DT> {
        self.discovery_table.blocking_write().into()
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
