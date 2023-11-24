use std::time::{Duration, Instant};
use super::TimeoutStrategy;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60 * 60 * 24);
pub struct ConstTimeoutStrategy {
    expire_after: Duration,
}

impl Default for ConstTimeoutStrategy {
    fn default() -> Self {
        Self {
            expire_after: DEFAULT_TIMEOUT,
        }
    }
}

impl<C> TimeoutStrategy<C> for ConstTimeoutStrategy {
    fn is_timed_out(&self, context: &C, time: &Instant) -> bool {
        Instant::now()
            .checked_duration_since(*time)
            .is_some_and(|d| d >= self.expire_after)
    }
}