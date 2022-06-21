use std::sync::Arc;

use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

#[cfg(feature = "tokio")]
pub use async_tokio_context::*;

use crate::domain::{
    Contact, DiscoveryTable, Interface, NeighborTable, NodeId, RoutingTable, DEFAULT_BUCKET_SIZE,
    DEFAULT_ID_SIZE,
};
use crate::messaging::{Message, MessageSender};
use crate::usecases::{Runtime, TimerId};

pub trait Context<
    RT,
    NT,
    DT,
    MS,
    RU,
    const ID_SIZE: usize = DEFAULT_ID_SIZE,
    const BUCKET_SIZE: usize = DEFAULT_BUCKET_SIZE,
>
{
    fn root_id(&self) -> &NodeId<ID_SIZE>;

    fn routing_table(&self) -> RwLockReadGuard<RT>;

    fn routing_table_mut(&self) -> RwLockWriteGuard<RT>;

    fn neighbor_table(&self) -> RwLockReadGuard<NT>;

    fn neighbor_table_mut(&self) -> RwLockWriteGuard<NT>;

    fn discovery_table(&self) -> RwLockReadGuard<DT>;

    fn discovery_table_mut(&self) -> RwLockWriteGuard<DT>;

    fn message_sender(&self) -> RwLockReadGuard<MS>;

    fn message_sender_mut(&self) -> RwLockWriteGuard<MS>;

    fn runtime(&self) -> RwLockReadGuard<RU>;

    fn runtime_mut(&self) -> RwLockWriteGuard<RU>;
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

impl<RT, NT, DT, MS, RU, const ID_SIZE: usize, const BUCKET_SIZE: usize>
    Context<RT, NT, DT, MS, RU, ID_SIZE, BUCKET_SIZE>
    for SyncContext<RT, NT, DT, MS, RU, ID_SIZE, BUCKET_SIZE>
{
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

    fn runtime_mut(&self) -> RwLockWriteGuard<RU> {
        self.runtime.blocking_write()
    }
}

#[derive(Debug, Clone)]
pub enum UseCaseEvent<const ID_SIZE: usize> {
    Message(Message<ID_SIZE>),
    Timer(TimerId),
}

#[cfg(feature = "tokio")]
mod async_tokio_context {
    use std::sync::Arc;

    use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

    use crate::domain::{
        Contact, DiscoveryTable, Interface, NeighborTable, NodeId, RoutingTable,
        DEFAULT_BUCKET_SIZE, DEFAULT_ID_SIZE,
    };
    use crate::messaging::MessageSender;
    use crate::usecases::{Context, Runtime};

    #[derive(Debug, Clone)]
    pub struct AsyncTokioContext<
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
        AsyncTokioContext<RT, NT, DT, MS, RU, ID_SIZE, BUCKET_SIZE>
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

    impl<RT, NT, DT, MS, RU, const ID_SIZE: usize, const BUCKET_SIZE: usize>
        Context<RT, NT, DT, MS, RU, ID_SIZE, BUCKET_SIZE>
        for AsyncTokioContext<RT, NT, DT, MS, RU, ID_SIZE, BUCKET_SIZE>
    {
        fn root_id(&self) -> &NodeId<ID_SIZE> {
            &self.root_id
        }

        fn routing_table(&self) -> RwLockReadGuard<RT> {
            tokio::runtime::Handle::current().block_on(self.routing_table.read())
        }

        fn routing_table_mut(&self) -> RwLockWriteGuard<RT> {
            tokio::runtime::Handle::current().block_on(self.routing_table.write())
        }

        fn neighbor_table(&self) -> RwLockReadGuard<NT> {
            tokio::runtime::Handle::current().block_on(self.neighbor_table.read())
        }

        fn neighbor_table_mut(&self) -> RwLockWriteGuard<NT> {
            tokio::runtime::Handle::current().block_on(self.neighbor_table.write())
        }

        fn discovery_table(&self) -> RwLockReadGuard<DT> {
            tokio::runtime::Handle::current().block_on(self.discovery_table.read())
        }

        fn discovery_table_mut(&self) -> RwLockWriteGuard<DT> {
            tokio::runtime::Handle::current().block_on(self.discovery_table.write())
        }

        fn message_sender(&self) -> RwLockReadGuard<MS> {
            tokio::runtime::Handle::current().block_on(self.message_sender.read())
        }

        fn message_sender_mut(&self) -> RwLockWriteGuard<MS> {
            tokio::runtime::Handle::current().block_on(self.message_sender.write())
        }

        fn runtime(&self) -> RwLockReadGuard<RU> {
            tokio::runtime::Handle::current().block_on(self.runtime.read())
        }

        fn runtime_mut(&self) -> RwLockWriteGuard<RU> {
            tokio::runtime::Handle::current().block_on(self.runtime.write())
        }
    }
}
