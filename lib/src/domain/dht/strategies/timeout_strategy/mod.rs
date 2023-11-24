use std::time::Instant;
pub use const_timeout_strategy::ConstTimeoutStrategy;

mod const_timeout_strategy;

pub trait TimeoutStrategy<C> {
    fn is_timed_out(&self, context: &C, time: &Instant) -> bool;
}