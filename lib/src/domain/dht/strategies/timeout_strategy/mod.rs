pub use const_timeout_strategy::ConstTimeoutStrategy;

mod const_timeout_strategy;

pub trait TimeoutStrategy {
    type Context;
    type Expirable;

    fn has_timed_out(&self, context: &Self::Context, expirable: &Self::Expirable) -> bool;
}