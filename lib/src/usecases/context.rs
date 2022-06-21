use std::ops::{Deref, DerefMut};
use std::sync::{Arc, RwLock};

#[cfg(feature = "tokio")]
pub use async_tokio_context::*;

use crate::domain::{
    Contact, DiscoveryTable, Interface, NeighborTable, NodeId, RoutingTable, DEFAULT_BUCKET_SIZE,
    DEFAULT_ID_SIZE,
};
use crate::messaging::{Message, MessageSender};
use crate::usecases::{Runtime, TimerId};

pub enum ReadGuard<'a, T> {
    Sync(std::sync::RwLockReadGuard<'a, T>),
    Async(tokio::sync::RwLockReadGuard<'a, T>),
}

impl<'a, T> From<std::sync::RwLockReadGuard<'a, T>> for ReadGuard<'a, T> {
    fn from(guard: std::sync::RwLockReadGuard<'a, T>) -> Self {
        Self::Sync(guard)
    }
}

impl<'a, T> From<tokio::sync::RwLockReadGuard<'a, T>> for ReadGuard<'a, T> {
    fn from(guard: tokio::sync::RwLockReadGuard<'a, T>) -> Self {
        Self::Async(guard)
    }
}

impl<'a, T> Deref for ReadGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Sync(guard) => guard.deref(),
            Self::Async(guard) => guard.deref(),
        }
    }
}

pub enum WriteGuard<'a, T> {
    Sync(std::sync::RwLockWriteGuard<'a, T>),
    Async(tokio::sync::RwLockWriteGuard<'a, T>),
}

impl<'a, T> From<std::sync::RwLockWriteGuard<'a, T>> for WriteGuard<'a, T> {
    fn from(guard: std::sync::RwLockWriteGuard<'a, T>) -> Self {
        Self::Sync(guard)
    }
}

impl<'a, T> From<tokio::sync::RwLockWriteGuard<'a, T>> for WriteGuard<'a, T> {
    fn from(guard: tokio::sync::RwLockWriteGuard<'a, T>) -> Self {
        Self::Async(guard)
    }
}

impl<'a, T> Deref for WriteGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Sync(guard) => guard.deref(),
            Self::Async(guard) => guard.deref(),
        }
    }
}

impl<'a, T> DerefMut for WriteGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Sync(guard) => guard.deref_mut(),
            Self::Async(guard) => guard.deref_mut(),
        }
    }
}

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

    fn routing_table(&self) -> ReadGuard<RT>;

    fn routing_table_mut(&self) -> WriteGuard<RT>;

    fn neighbor_table(&self) -> ReadGuard<NT>;

    fn neighbor_table_mut(&self) -> WriteGuard<NT>;

    fn discovery_table(&self) -> ReadGuard<DT>;

    fn discovery_table_mut(&self) -> WriteGuard<DT>;

    fn message_sender(&self) -> ReadGuard<MS>;

    fn message_sender_mut(&self) -> WriteGuard<MS>;

    fn runtime(&self) -> &RU;
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
    runtime: RU,
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
            runtime,
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

#[derive(Debug, Clone)]
pub enum UseCaseEvent<const ID_SIZE: usize> {
    Message(Message<ID_SIZE>),
    Timer(TimerId),
}

#[cfg(feature = "tokio")]
mod async_tokio_context {
    use std::sync::Arc;

    use tokio::sync::RwLock;

    use crate::domain::{
        Contact, DiscoveryTable, Interface, NeighborTable, NodeId, RoutingTable,
        DEFAULT_BUCKET_SIZE, DEFAULT_ID_SIZE,
    };
    use crate::messaging::MessageSender;
    use crate::usecases::broadcaster::Broadcaster;
    use crate::usecases::{AsyncTokioRuntime, Context, ReadGuard, WriteGuard};

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
        runtime: RU,
    }

    impl<RT, NT, DT, MS, B, const ID_SIZE: usize, const BUCKET_SIZE: usize>
        AsyncTokioContext<RT, NT, DT, MS, AsyncTokioRuntime<B, ID_SIZE>, ID_SIZE, BUCKET_SIZE>
    where
        RT: RoutingTable<ID_SIZE, BUCKET_SIZE>,
        for<'a> &'a RT: IntoIterator<Item = &'a Contact<ID_SIZE>>,
        NT: NeighborTable<ID_SIZE>,
        for<'a> &'a NT: IntoIterator<Item = (&'a NodeId<ID_SIZE>, &'a Interface)>,
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
            runtime: AsyncTokioRuntime<B, ID_SIZE>,
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
        Context<RT, NT, DT, MS, AsyncTokioRuntime<B, ID_SIZE>, ID_SIZE, BUCKET_SIZE>
        for AsyncTokioContext<RT, NT, DT, MS, AsyncTokioRuntime<B, ID_SIZE>, ID_SIZE, BUCKET_SIZE>
    where
        B: Broadcaster<ID_SIZE>,
    {
        fn root_id(&self) -> &NodeId<ID_SIZE> {
            &self.root_id
        }

        fn routing_table(&self) -> ReadGuard<RT> {
            self.runtime
                .runtime()
                .block_on(self.routing_table.read())
                .into()
        }

        fn routing_table_mut(&self) -> WriteGuard<RT> {
            self.runtime
                .runtime()
                .block_on(self.routing_table.write())
                .into()
        }

        fn neighbor_table(&self) -> ReadGuard<NT> {
            self.runtime
                .runtime()
                .block_on(self.neighbor_table.read())
                .into()
        }

        fn neighbor_table_mut(&self) -> WriteGuard<NT> {
            self.runtime
                .runtime()
                .block_on(self.neighbor_table.write())
                .into()
        }

        fn discovery_table(&self) -> ReadGuard<DT> {
            self.runtime
                .runtime()
                .block_on(self.discovery_table.read())
                .into()
        }

        fn discovery_table_mut(&self) -> WriteGuard<DT> {
            self.runtime
                .runtime()
                .block_on(self.discovery_table.write())
                .into()
        }

        fn message_sender(&self) -> ReadGuard<MS> {
            self.runtime
                .runtime()
                .block_on(self.message_sender.read())
                .into()
        }

        fn message_sender_mut(&self) -> WriteGuard<MS> {
            self.runtime
                .runtime()
                .block_on(self.message_sender.write())
                .into()
        }

        fn runtime(&self) -> &AsyncTokioRuntime<B, ID_SIZE> {
            &self.runtime
        }
    }
}
