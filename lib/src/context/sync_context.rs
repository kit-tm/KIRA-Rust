use std::sync::{Arc, RwLock};

use crate::context::{Context, ReadGuard, WriteGuard};
use crate::domain::NodeId;

#[derive(Debug, Clone)]
pub struct SyncContext<RT, NT, MS, RU> {
    root_id: NodeId,
    routing_table: Arc<RwLock<RT>>,
    neighbor_table: Arc<RwLock<NT>>,
    message_sender: Arc<RwLock<MS>>,
    runtime: RU,
}

impl<RT, NT, MS, RU> SyncContext<RT, NT, MS, RU> {
    /// Creates a new [Context].
    pub fn new(
        root_id: NodeId,
        routing_table: RT,
        neighbor_table: NT,
        message_sender: MS,
        runtime: RU,
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

impl<RT, NT, MS, RU> Context for SyncContext<RT, NT, MS, RU> {
    type RoutingTable = RT;
    type NeighborTable = NT;
    type MessageSender = MS;
    type Runtime = RU;

    fn root_id(&self) -> &NodeId {
        &self.root_id
    }

    fn routing_table(&self) -> ReadGuard<RT> {
        self.routing_table
            .read()
            .expect("failed to get lock")
            .into()
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
