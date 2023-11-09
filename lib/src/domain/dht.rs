use std::error::Error;
use std::task::Context;
use std::time::Instant;

pub enum DHTData {
    Single(u8),
    Slice([u8]),
    List(u8),
}

pub trait HashTable<H, D> {
    type StoreError: Error;
    fn store(handle: H, data: D) -> Result<(), Self::StoreError>;
    fn fetch(handle: H) -> Some<D>;
    fn delete(handle: H);
}

pub trait TimeoutStrategy<C> {
    fn is_timed_out(&self, context: C, time: Instant) -> bool;
}

pub trait Expiring<C> {
    fn expire();
    fn expire_with_strategy(&self, strategy: impl TimeoutStrategy<C>);
}