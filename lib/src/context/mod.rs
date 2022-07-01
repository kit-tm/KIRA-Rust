use std::ops::{Deref, DerefMut};

pub use sync_context::*;
#[cfg(feature = "tokio")]
pub use tokio_context::*;

use crate::domain::{NeighborTable, NodeId};

pub mod sync_context;
#[cfg(feature = "tokio")]
pub mod tokio_context;

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

/// Context a UseCase runs in.
///
/// Provides access to the shared global state of the [Node].
///
/// The methods return either a [ReadGuard] or [WriteGuard] depending on the mutability
/// of the shared state returned.
/// These are used to support async as well as sync runtimes which may use different
/// locks (e.g. [tokio::sync::RwLock], [std::sync::RwLock]) to access shared resources.
///
/// The support for different runtime environments (async vs. sync) is determined by the
/// concrete implementation.
pub trait UseCaseContext {
    type RoutingTable: Sized;
    type MessageSender: Sized;
    type Runtime: Sized;
    type InsertionStrategy: Sized;

    fn root_id(&self) -> &NodeId;

    fn routing_table(&self) -> ReadGuard<Self::RoutingTable>;

    fn routing_table_mut(&self) -> WriteGuard<Self::RoutingTable>;

    fn routing_table_insertion_strategy(&self) -> WriteGuard<Self::InsertionStrategy>;

    fn neighbor_table(&self) -> ReadGuard<NeighborTable>;

    fn neighbor_table_mut(&self) -> WriteGuard<NeighborTable>;

    fn message_sender(&self) -> ReadGuard<Self::MessageSender>;

    fn message_sender_mut(&self) -> WriteGuard<Self::MessageSender>;

    fn runtime(&self) -> &Self::Runtime;
}
