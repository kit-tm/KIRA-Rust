use std::marker::PhantomData;
use std::time::{Duration, Instant};
use crate::domain::dht::TimedValue;
use super::TimeoutStrategy;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60 * 60 * 24);
pub struct ConstTimeoutStrategy<C, D> {
    _c: PhantomData<C>,
    _d: PhantomData<D>,
    expire_after: Duration,
}

impl<C, D> Default for ConstTimeoutStrategy<C, D> {
    fn default() -> Self {
        Self {
            _c: PhantomData::default(),
            _d: PhantomData::default(),
            expire_after: DEFAULT_TIMEOUT,
        }
    }
}

impl<C, D> TimeoutStrategy for ConstTimeoutStrategy<C, D> {
    type Context = C;
    type Expirable = TimedValue<D>;

    fn has_timed_out(&self, context: &Self::Context, expirable: &Self::Expirable) -> bool {
        Instant::now()
            .checked_duration_since(expirable.time.clone())
            .is_some_and(|d| d >= self.expire_after)
    }
}