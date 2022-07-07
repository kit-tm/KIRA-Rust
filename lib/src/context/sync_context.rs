use std::sync::{Arc, RwLock};

use crate::context::{ReadGuard, UseCaseContext, WriteGuard};
use crate::domain::{NodeId, PNTable};

#[derive(Debug, Clone)]
pub struct SyncContext<RT, MS, RU, IS> {
    root_id: NodeId,
    routing_table: Arc<RwLock<RT>>,
    insertion_strategy: Arc<RwLock<IS>>,
    pn_table: Arc<RwLock<PNTable>>,
    message_sender: Arc<RwLock<MS>>,
    runtime: RU,
}

impl<RT, MS, RU, IS> SyncContext<RT, MS, RU, IS> {
    /// Creates a new [Context].
    pub fn new(
        root_id: NodeId,
        routing_table: RT,
        pn_table: PNTable,
        insertion_strategy: IS,
        message_sender: MS,
        runtime: RU,
    ) -> Self {
        Self {
            root_id,
            routing_table: Arc::new(RwLock::new(routing_table)),
            insertion_strategy: Arc::new(RwLock::new(insertion_strategy)),
            pn_table: Arc::new(RwLock::new(pn_table)),
            message_sender: Arc::new(RwLock::new(message_sender)),
            runtime,
        }
    }
}

impl<RT, MS, RU, IS> UseCaseContext for SyncContext<RT, MS, RU, IS> {
    type RoutingTable = RT;
    type MessageSender = MS;
    type Runtime = RU;
    type InsertionStrategy = IS;

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

    fn routing_table_insertion_strategy(&self) -> WriteGuard<IS> {
        self.insertion_strategy
            .write()
            .expect("failed to get lock")
            .into()
    }

    fn pn_table(&self) -> ReadGuard<PNTable> {
        self.pn_table.read().expect("faile to get lock").into()
    }

    fn pn_table_mut(&self) -> WriteGuard<PNTable> {
        self.pn_table.write().expect("faile to get lock").into()
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
