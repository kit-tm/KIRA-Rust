use std::error::Error;
use std::time::Instant;

pub mod expiring_hash_table;

pub const DEFAULT_SLICE_SIZE: usize = 8;

#[derive(Debug)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum DHTData<T, const SLICE_SIZE: usize = DEFAULT_SLICE_SIZE> {
    Single(T),
    Slice([T; SLICE_SIZE]),
    Set(T),
}

pub trait HashTable<H, D> {
    type StoreErr;
    type StoreOK;
    type FetchErr;

    fn store(&mut self, handle: H, data: D) -> Result<Self::StoreOK, Self::StoreErr>;
    fn fetch(&self, handle: &H) -> Result<D, Self::FetchErr>;
}

pub trait TimeoutStrategy<C> {
    fn is_timed_out(&self, context: &C, time: &Instant) -> bool;
}

pub trait Expiring<C> {
    fn expire(&mut self);
    fn expire_with_strategy(&mut self, strategy: &impl TimeoutStrategy<C>);
}