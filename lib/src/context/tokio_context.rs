use std::sync::Arc;

use tokio::sync::RwLock;

use crate::broadcaster::Broadcaster;
use crate::context::{Context, ReadGuard, WriteGuard};
use crate::domain::{
    Contact, DiscoveryTable, NeighborTable, NodeId, Port, RoutingTable, DEFAULT_BUCKET_SIZE,
};
use crate::messaging::sender::ProtocolMessageSender;
use crate::runtime::TokioRuntime;

#[derive(Debug, Clone)]
pub struct TokioContext<RT, NT, DT, MS, RU, const BUCKET_SIZE: usize = DEFAULT_BUCKET_SIZE> {
    root_id: NodeId,
    routing_table: Arc<RwLock<RT>>,
    neighbor_table: Arc<RwLock<NT>>,
    discovery_table: Arc<RwLock<DT>>,
    message_sender: Arc<RwLock<MS>>,
    runtime: RU,
}

impl<RT, NT, DT, MS, B, const BUCKET_SIZE: usize>
    TokioContext<RT, NT, DT, MS, TokioRuntime<B>, BUCKET_SIZE>
where
    RT: RoutingTable<BUCKET_SIZE>,
    for<'a> &'a RT: IntoIterator<Item = &'a Contact>,
    NT: NeighborTable,
    for<'a> &'a NT: IntoIterator<Item = (&'a NodeId, &'a Port)>,
    DT: DiscoveryTable,
    MS: ProtocolMessageSender,
    B: Broadcaster,
{
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

impl<RT, NT, DT, MS, B, const BUCKET_SIZE: usize>
    Context<RT, NT, DT, MS, TokioRuntime<B>, BUCKET_SIZE>
    for TokioContext<RT, NT, DT, MS, TokioRuntime<B>, BUCKET_SIZE>
where
    B: Broadcaster,
{
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
