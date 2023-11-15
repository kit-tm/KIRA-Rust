use std::error::Error;
use std::time::Instant;
const DEFAULT_SLICE_SIZE: usize = 8;

#[derive(Debug)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum DHTData<const SLICE_SIZE: usize = DEFAULT_SLICE_SIZE> {
    Single(u8),
    Slice([u8; SLICE_SIZE]),
    List(u8),
}

pub trait HashTable<H, D> {
    type StoreErr: Error;
    type StoreOK;
    type FetchErr: Error;

    fn store(&self, handle: H, data: D) -> Result<Self::StoreOK, Self::StoreErr>;
    fn fetch(&self, handle: H) -> Result<D, Self::FetchErr>;
    fn delete(&self, handle: H);
}

pub trait TimeoutStrategy<C> {
    fn is_timed_out(&self, context: C, time: Instant) -> bool;
}

pub trait Expiring<C> {
    fn expire();
    fn expire_with_strategy(&self, strategy: impl TimeoutStrategy<C>);
}