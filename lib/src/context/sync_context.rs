use crate::context::{Context, ReadGuard, WriteGuard};
use crate::domain::{
    Contact, DiscoveryTable, NeighborTable, NodeId, Port, RoutingTable, DEFAULT_BUCKET_SIZE,
};
use crate::messaging::sender::MessageSender;
use crate::runtime::Runtime;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone)]
pub struct SyncContext<RT, NT, DT, MS, RU, const BUCKET_SIZE: usize = DEFAULT_BUCKET_SIZE> {
    root_id: NodeId,
    routing_table: Arc<RwLock<RT>>,
    neighbor_table: Arc<RwLock<NT>>,
    discovery_table: Arc<RwLock<DT>>,
    message_sender: Arc<RwLock<MS>>,
    runtime: RU,
}

impl<RT, NT, DT, MS, RU, const BUCKET_SIZE: usize> SyncContext<RT, NT, DT, MS, RU, BUCKET_SIZE>
where
    RT: RoutingTable<BUCKET_SIZE>,
    for<'a> &'a RT: IntoIterator<Item = &'a Contact>,
    NT: NeighborTable,
    for<'a> &'a NT: IntoIterator<Item = (&'a NodeId, &'a Port)>,
    DT: DiscoveryTable,
    MS: MessageSender,
    RU: Runtime,
{
    /// Creates a new [Context].
    pub fn new(
        root_id: NodeId,
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
            runtime,
        }
    }
}

impl<RT, NT, DT, MS, RU, const BUCKET_SIZE: usize> Context<RT, NT, DT, MS, RU, BUCKET_SIZE>
    for SyncContext<RT, NT, DT, MS, RU, BUCKET_SIZE>
{
    fn root_id(&self) -> &NodeId {
        &self.root_id
    }

    fn routing_table(&self) -> ReadGuard<RT> {
        self.routing_table.read().expect("faile to get lock").into()
    }

    fn routing_table_mut(&self) -> WriteGuard<RT> {
        self.routing_table
            .write()
            .expect("faile to get lock")
            .into()
    }

    fn neighbor_table(&self) -> ReadGuard<NT> {
        self.neighbor_table
            .read()
            .expect("faile to get lock")
            .into()
    }

    fn neighbor_table_mut(&self) -> WriteGuard<NT> {
        self.neighbor_table
            .write()
            .expect("faile to get lock")
            .into()
    }

    fn discovery_table(&self) -> ReadGuard<DT> {
        self.discovery_table
            .read()
            .expect("faile to get lock")
            .into()
    }

    fn discovery_table_mut(&self) -> WriteGuard<DT> {
        self.discovery_table
            .write()
            .expect("faile to get lock")
            .into()
    }

    fn message_sender(&self) -> ReadGuard<MS> {
        self.message_sender
            .read()
            .expect("faile to get lock")
            .into()
    }

    fn message_sender_mut(&self) -> WriteGuard<MS> {
        self.message_sender
            .write()
            .expect("faile to get lock")
            .into()
    }

    fn runtime(&self) -> &RU {
        &self.runtime
    }
}
