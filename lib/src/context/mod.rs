//! [UseCaseContext] related structures and traits.

use std::cell::{Ref, RefMut};
use std::collections::HashSet;
use std::ops::{Deref, DerefMut};

pub use sync_context::*;
#[cfg(feature = "tokio")]
pub use tokio_context::*;

use crate::domain::{NodeId, NotVia, PNTable};

pub mod sync_context;
#[cfg(feature = "tokio")]
pub mod tokio_context;

/// Delegates read access to a type through internally supported RAII types.
/// Conversion of supported RAII types into a [ReadGuard] is done through the [From] trait
/// implementations.
pub enum ReadGuard<'a, T> {
    Sync(Ref<'a, T>),
    Async(tokio::sync::RwLockReadGuard<'a, T>),
}

impl<'a, T> From<Ref<'a, T>> for ReadGuard<'a, T> {
    fn from(guard: Ref<'a, T>) -> Self {
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

/// Delegates write access to a type through internally supported RAII types.
/// Conversion of supported RAII types into a [WriteGuard] is done through the [From] trait
/// implementations.
pub enum WriteGuard<'a, T> {
    Sync(RefMut<'a, T>),
    Async(tokio::sync::RwLockWriteGuard<'a, T>),
}

impl<'a, T> From<RefMut<'a, T>> for WriteGuard<'a, T> {
    fn from(guard: RefMut<'a, T>) -> Self {
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

/// Configuration Wrapper for all dependencies of a [UseCaseContext].
pub struct ContextConfig<RT, MS, RU, IS, FT> {
    pub root_id: NodeId,
    pub routing_table: RT,
    pub message_sender: MS,
    pub runtime: RU,
    pub insertion_strategy: IS,
    pub pn_table: PNTable,
    pub forwarding_tables: FT,
    pub not_via: HashSet<NotVia>,
}

/// Context a UseCase runs in.
///
/// Provides access to the shared global state of the node.
///
/// The methods return either a [ReadGuard] or [WriteGuard] depending on the mutability
/// of the shared state returned.
/// These are used to support async as well as sync runtimes which may use different
/// locks (e.g. [tokio::sync::RwLock], [std::sync::RwLock]) to access shared resources.
///
/// The support for different runtime environments (async vs. sync) is determined by the
/// concrete implementation.
///
/// NotVia Data represents links and nodes in the routing table which are not longer functional.
///
/// NotVia Data of other nodes will only be added if they affect contacts in the own routing table.
pub trait UseCaseContext {
    type RoutingTable: Sized;
    type MessageSender: Sized;
    type Runtime: Sized;
    type InsertionStrategy: Sized;
    type ForwardingTables: Sized;

    fn new(
        config: ContextConfig<
            Self::RoutingTable,
            Self::MessageSender,
            Self::Runtime,
            Self::InsertionStrategy,
            Self::ForwardingTables,
        >,
    ) -> Self;

    fn root_id(&self) -> &NodeId;

    fn routing_table(&self) -> ReadGuard<'_, Self::RoutingTable>;

    fn routing_table_mut(&self) -> WriteGuard<'_, Self::RoutingTable>;

    fn routing_table_insertion_strategy(&self) -> WriteGuard<'_, Self::InsertionStrategy>;

    fn pn_table(&self) -> ReadGuard<'_, PNTable>;

    fn pn_table_mut(&self) -> WriteGuard<'_, PNTable>;

    fn message_sender(&self) -> ReadGuard<'_, Self::MessageSender>;

    fn message_sender_mut(&self) -> WriteGuard<'_, Self::MessageSender>;

    fn forwarding_tables(&self) -> ReadGuard<'_, Self::ForwardingTables>;

    fn forwarding_tables_mut(&self) -> WriteGuard<'_, Self::ForwardingTables>;

    fn runtime(&self) -> &Self::Runtime;

    fn not_via(&self) -> ReadGuard<'_, HashSet<NotVia>>;

    fn not_via_mut(&self) -> WriteGuard<'_, HashSet<NotVia>>;

    fn to_api_model(&self) -> crate::domain::api::RoutingTable;
}
