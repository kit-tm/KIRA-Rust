use std::sync::Arc;

use tokio::sync::RwLock;

use crate::broadcaster::Broadcaster;
use crate::context::{Context, ReadGuard, WriteGuard};
use crate::domain::{
    Contact, DiscoveryTable, Port, NeighborTable, NodeId, RoutingTable, DEFAULT_BUCKET_SIZE,
    DEFAULT_ID_SIZE,
};
use crate::messaging::sender::MessageSender;
use crate::runtime::TokioRuntime;

#[derive(Debug, Clone)]
pub struct TokioContext<
    RT,
    NT,
    DT,
    MS,
    RU,
    const ID_SIZE: usize = DEFAULT_ID_SIZE,
    const BUCKET_SIZE: usize = DEFAULT_BUCKET_SIZE,
> {
    root_id: NodeId<ID_SIZE>,
    routing_table: Arc<RwLock<RT>>,
    neighbor_table: Arc<RwLock<NT>>,
    discovery_table: Arc<RwLock<DT>>,
    message_sender: Arc<RwLock<MS>>,
    runtime: RU,
}

impl<RT, NT, DT, MS, B, const ID_SIZE: usize, const BUCKET_SIZE: usize>
    TokioContext<RT, NT, DT, MS, TokioRuntime<B, ID_SIZE>, ID_SIZE, BUCKET_SIZE>
where
    RT: RoutingTable<ID_SIZE, BUCKET_SIZE>,
    for<'a> &'a RT: IntoIterator<Item = &'a Contact<ID_SIZE>>,
    NT: NeighborTable<ID_SIZE>,
    for<'a> &'a NT: IntoIterator<Item = (&'a NodeId<ID_SIZE>, &'a Port)>,
    DT: DiscoveryTable<ID_SIZE>,
    MS: MessageSender<ID_SIZE>,
    B: Broadcaster<ID_SIZE>,
{
    /// Creates a new [Context].
    pub fn new(
        root_id: NodeId<ID_SIZE>,
        routing_table: RT,
        neighbor_table: NT,
        discovery_table: DT,
        message_sender: MS,
        runtime: TokioRuntime<B, ID_SIZE>,
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

impl<RT, NT, DT, MS, B, const ID_SIZE: usize, const BUCKET_SIZE: usize>
    Context<RT, NT, DT, MS, TokioRuntime<B, ID_SIZE>, ID_SIZE, BUCKET_SIZE>
    for TokioContext<RT, NT, DT, MS, TokioRuntime<B, ID_SIZE>, ID_SIZE, BUCKET_SIZE>
where
    B: Broadcaster<ID_SIZE>,
{
    fn root_id(&self) -> &NodeId<ID_SIZE> {
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

    fn runtime(&self) -> &TokioRuntime<B, ID_SIZE> {
        &self.runtime
    }
}
