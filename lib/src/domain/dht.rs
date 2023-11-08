use std::error::Error;
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

pub trait TimeoutStrategy {
    type Context;

    fn is_timed_out(&self, context: &Self::Context, time: Instant) -> bool;
}

pub trait Expiring {
    type Context;

    fn expire();
    fn expire_with_strategy(&self, strategy: impl TimeoutStrategy<Context=&Self::Context>);
}