use std::marker::PhantomData;
use std::time::{Duration, Instant};
use crate::domain::dht::TimedValue;
use super::TimeoutStrategy;

/// Default [Duration] of time after which a value is decided to be timed out.
///
/// This value is used by [ConstTimeoutStrategy::default]
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60 * 60 * 24);


/// Implementation of the [TimeoutStrategy] on which values run out after a given duration.
///
/// This strategy doesn't require any context
/// but the values must be a [TimedValue] to provide the [Instant] on which they are created.
#[derive(Debug, Clone)]
pub struct ConstTimeoutStrategy<C, D> {
    _c: PhantomData<C>,
    _d: PhantomData<D>,
    expire_after: Duration,
}

impl<C, D> ConstTimeoutStrategy<C, D> {
    /// Creates a new instance of [ConstTimeoutStrategy] with the specified expiration duration.
    ///
    /// # Parameters
    ///
    /// - `expire_after`: The duration after which the data will be considered expired.
    ///
    /// # Returns
    ///
    /// The new [ConstTimeoutStrategy] instance with the specified expiration duration.
    pub fn with_expire_duration(expire_after: Duration) -> Self {
        Self {
            _c: PhantomData,
            _d: PhantomData,
            expire_after,
        }
    }
}

impl<C, D> Default for ConstTimeoutStrategy<C, D> {
    /// Returns a new instance of [ConstTimeoutStrategy] with the [DEFAULT_TIMEOUT].
    ///
    /// If you want to create a custom [ConstTimeoutStrategy] you can use [ConstTimeoutStrategy::with_expire_duration].
    fn default() -> Self {
        Self::with_expire_duration(DEFAULT_TIMEOUT)
    }
}

impl<C, D> TimeoutStrategy for ConstTimeoutStrategy<C, D> {
    type Context = C;
    type Expirable = TimedValue<D>;

    fn has_timed_out(&self, _context: &Self::Context, expirable: &Self::Expirable) -> bool {
        Instant::now()
            .checked_duration_since(expirable.timestamp)
            .is_some_and(|d| d >= self.expire_after)
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Sub;
    use super::*;
    use super::super::TimeoutStrategy;

    #[test]
    fn default_expire_duration() {
        assert_eq!(ConstTimeoutStrategy::<(), ()>::default().expire_after, DEFAULT_TIMEOUT);
    }

    #[test]
    fn expire_after_duration() {
        let strategy = ConstTimeoutStrategy::default();

        let mut expired_value = TimedValue::new("tested");
        expired_value.timestamp = Instant::now().sub(DEFAULT_TIMEOUT).sub(Duration::from_secs(42));

        assert!(strategy.has_timed_out(&(), &expired_value), "Value hasn't expired: {expired_value:?}");
    }

    #[test]
    fn dont_expire_fresh_value() {
        let strategy = ConstTimeoutStrategy::default();

        let mut expired_value = TimedValue::new("tested");
        expired_value.timestamp = Instant::now().sub(DEFAULT_TIMEOUT / 2);

        assert!(!strategy.has_timed_out(&(), &expired_value), "Value has expired: {expired_value:?}");
    }
}