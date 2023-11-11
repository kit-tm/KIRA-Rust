use std::error::Error;
use std::time::Instant;

pub enum DHTData {
    Single(u8),
    Slice([u8]),
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