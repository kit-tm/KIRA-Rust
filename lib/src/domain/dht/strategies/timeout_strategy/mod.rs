pub use const_timeout_strategy::ConstTimeoutStrategy;
pub use const_timeout_strategy::DEFAULT_TIMEOUT;
#[cfg(test)]
pub use tagged_timeout_strategy::{TaggedTimeoutStrategy, TaggedValue};
use super::super::expiring::Expiring;

mod const_timeout_strategy;
mod tagged_timeout_strategy;

/// This strategy decides if a given value is expired.
///
/// The strategy can use an external [Self::Context] to decide if the value has expired.
/// 
/// To see the difference between this trait and the [Expiring] trait see the 
/// according section in the [Expiring] trait.
pub trait TimeoutStrategy {
    /// Context aiding the decision if a value has timed out.
    ///
    /// This can be something like the `NodeId` of the running node or just the Unit type `()`.
    type Context;

    /// The type of value the [TimeoutStrategy] rules on is expired.
    type Expirable;

    /// This function returns if a given `expirable` has timed out.
    ///
    /// # Arguments
    ///
    /// * `context`: Context aiding the decision if the expirable is expired
    /// * `expirable`: Value on which to rule
    ///
    /// # Returns
    ///
    /// `true` if the value has timed out. `false` otherwise
    fn has_timed_out(&self, context: &Self::Context, expirable: &Self::Expirable) -> bool;
}