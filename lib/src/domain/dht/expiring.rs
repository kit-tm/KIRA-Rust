use crate::domain::dht::strategies::timeout_strategy::TimeoutStrategy;

pub trait Expiring {
    type Context;
    type Result;

    fn expire(&mut self, context: &Self::Context) -> Self::Result;
}
