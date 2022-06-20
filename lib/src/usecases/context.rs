use std::sync::Arc;
use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::domain::{
    Contact, DiscoveryTable, Interface, NeighborTable, NodeId, RoutingTable, DEFAULT_BUCKET_SIZE,
    DEFAULT_ID_SIZE,
};
use crate::messaging::{Message, MessageSender};
use crate::usecases::{Runtime, TimerId};

pub trait Context<const ID_SIZE: usize = DEFAULT_ID_SIZE> {
    type RoutingTable;
    type NeighborTable;
    type DiscoveryTable;
    type MessageSender;
    type Runtime;

    fn root_id(&self) -> &NodeId<ID_SIZE>;

    fn routing_table(&self) -> RwLockReadGuard<Self::RoutingTable>;

    fn routing_table_mut(&self) -> RwLockWriteGuard<Self::RoutingTable>;

    fn neighbor_table(&self) -> RwLockReadGuard<Self::NeighborTable>;

    fn neighbor_table_mut(&self) -> RwLockWriteGuard<Self::NeighborTable>;

    fn discovery_table(&self) -> RwLockReadGuard<Self::DiscoveryTable>;

    fn discovery_table_mut(&self) -> RwLockWriteGuard<Self::DiscoveryTable>;

    fn message_sender(&self) -> RwLockReadGuard<Self::MessageSender>;

    fn message_sender_mut(&self) -> RwLockWriteGuard<Self::MessageSender>;

    fn runtime(&self) -> RwLockReadGuard<Self::Runtime>;
}

#[derive(Debug, Clone)]
pub struct SyncContext<
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
    runtime: Arc<RwLock<RU>>,
}

impl<RT, NT, DT, MS, RU, const ID_SIZE: usize, const BUCKET_SIZE: usize>
    SyncContext<RT, NT, DT, MS, RU, ID_SIZE, BUCKET_SIZE>
where
    RT: RoutingTable<ID_SIZE, BUCKET_SIZE>,
    for<'a> &'a RT: IntoIterator<Item = &'a Contact<ID_SIZE>>,
    NT: NeighborTable<ID_SIZE>,
    for<'a> &'a NT: IntoIterator<Item = (&'a NodeId<ID_SIZE>, &'a Interface)>,
    DT: DiscoveryTable<ID_SIZE>,
    MS: MessageSender<ID_SIZE>,
    RU: Runtime,
{
    /// Creates a new [Context].
    pub fn new(
        root_id: NodeId<ID_SIZE>,
        routing_table: RT,
        neighbor_table: NT,
        discovery_table: DT,
        message_sender: MS,
        runtime: RU,
    ) -> Self {
        Self {
            root_id,
            routing_table: Arc::new(RwLock::new(routing_table)),
            neighbor_table: Arc::new(RwLock::new(neighbor_table)),
            discovery_table: Arc::new(RwLock::new(discovery_table)),
            message_sender: Arc::new(RwLock::new(message_sender)),
            runtime: Arc::new(RwLock::new(runtime)),
        }
    }
}

impl<RT, NT, DT, MS, RU, const ID_SIZE: usize, const BUCKET_SIZE: usize> Context<ID_SIZE>
    for SyncContext<RT, NT, DT, MS, RU, ID_SIZE, BUCKET_SIZE>
{
    type RoutingTable = RT;
    type NeighborTable = NT;
    type DiscoveryTable = DT;
    type MessageSender = MS;
    type Runtime = RU;

    fn root_id(&self) -> &NodeId<ID_SIZE> {
        &self.root_id
    }

    fn routing_table(&self) -> RwLockReadGuard<RT> {
        self.routing_table.blocking_read()
    }

    fn routing_table_mut(&self) -> RwLockWriteGuard<RT> {
        self.routing_table.blocking_write()
    }

    fn neighbor_table(&self) -> RwLockReadGuard<NT> {
        self.neighbor_table.blocking_read()
    }

    fn neighbor_table_mut(&self) -> RwLockWriteGuard<NT> {
        self.neighbor_table.blocking_write()
    }

    fn discovery_table(&self) -> RwLockReadGuard<DT> {
        self.discovery_table.blocking_read()
    }

    fn discovery_table_mut(&self) -> RwLockWriteGuard<DT> {
        self.discovery_table.blocking_write()
    }

    fn message_sender(&self) -> RwLockReadGuard<MS> {
        self.message_sender.blocking_read()
    }

    fn message_sender_mut(&self) -> RwLockWriteGuard<MS> {
        self.message_sender.blocking_write()
    }

    fn runtime(&self) -> RwLockReadGuard<RU> {
        self.runtime.blocking_read()
    }
}

#[derive(Debug, Clone)]
pub enum UseCaseEvent<const ID_SIZE: usize> {
    Message(Message<ID_SIZE>),
    Timer(TimerId),
}
