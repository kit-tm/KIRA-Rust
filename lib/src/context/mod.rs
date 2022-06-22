use std::ops::{Deref, DerefMut};

pub use sync_context::*;
#[cfg(feature = "tokio")]
pub use tokio_context::*;

use crate::domain::{NodeId, DEFAULT_BUCKET_SIZE, DEFAULT_ID_SIZE};

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
