pub use const_timeout_strategy::ConstTimeoutStrategy;
pub use const_timeout_strategy::DEFAULT_TIMEOUT;
#[cfg(test)]
pub use immediate_timeout_strategy::{TaggedTimeoutStrategy, TaggedValue};

mod const_timeout_strategy;
mod immediate_timeout_strategy;

pub trait TimeoutStrategy {
    type Context;
    type Expirable;

    fn has_timed_out(&self, context: &Self::Context, expirable: &Self::Expirable) -> bool;
}