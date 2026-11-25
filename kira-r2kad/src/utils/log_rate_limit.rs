//! Rate limiting of log messages that may be emitted for every protocol message.
//!
//! When a node is overloaded or its underlay neighbors fail, such messages can be emitted
//! thousands of times per second. Formatting them (often with a Debug dump of the protocol
//! message) and writing them then adds to the load that caused them, so the node can't recover.
//! See [rate_limited](crate::rate_limited).

use std::{
    sync::{
        OnceLock,
        atomic::{
            AtomicU64,
            Ordering,
        },
    },
    time::{
        Duration,
        Instant,
    },
};

/// Default interval between two log messages of the same call site.
pub const LOG_RATE_LIMIT_INTERVAL: Duration = Duration::from_secs(1);

/// Admits one event per interval and counts the others.
///
/// Usually used through [rate_limited](crate::rate_limited), which keeps one limiter per call
/// site.
#[derive(Debug, Default)]
pub struct LogRateLimiter {
    /// Earliest time of the next admitted event, in milliseconds since [process_start].
    next_ms: AtomicU64,
    /// Events not admitted since the last admitted one.
    suppressed: AtomicU64,
}

impl LogRateLimiter {
    pub const fn new() -> Self {
        Self {
            next_ms: AtomicU64::new(0),
            suppressed: AtomicU64::new(0),
        }
    }

    /// Returns the number of events suppressed since the last admitted one if this event is
    /// admitted, i.e., if `interval` passed since then; otherwise counts it and returns [None].
    pub fn check(&self, interval: Duration) -> Option<u64> {
        self.check_at(process_start().elapsed(), interval)
    }

    /// [Self::check] at the time `now` since an arbitrary, fixed start.
    pub fn check_at(&self, now: Duration, interval: Duration) -> Option<u64> {
        let now_ms = u64::try_from(now.as_millis()).unwrap_or(u64::MAX);
        let next_ms = self.next_ms.load(Ordering::Relaxed);
        let interval_ms = u64::try_from(interval.as_millis()).unwrap_or(u64::MAX);
        if now_ms >= next_ms
            && self
                .next_ms
                .compare_exchange(
                    next_ms,
                    now_ms.saturating_add(interval_ms),
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .is_ok()
        {
            Some(self.suppressed.swap(0, Ordering::Relaxed))
        } else {
            self.suppressed.fetch_add(1, Ordering::Relaxed);
            None
        }
    }
}

/// Start of the time base of all [LogRateLimiter]s.
fn process_start() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

/// Evaluates the expression (usually a log macro) at most once per interval at this call site.
///
/// The binding receives the number of times it was skipped since its last evaluation, to be
/// included in the log message. The arguments of skipped log messages are not evaluated, so
/// expensive Debug output costs nothing while suppressed.
///
/// ```
/// use std::time::Duration;
///
/// for _ in 0..1000 {
///     kira_r2kad::rate_limited!(|suppressed| tracing::warn!(suppressed, "Dropping message"));
///     kira_r2kad::rate_limited!(Duration::from_secs(10), |suppressed| {
///         tracing::warn!(suppressed, "Dropping another message")
///     });
/// }
/// ```
#[macro_export]
macro_rules! rate_limited {
    (|$suppressed:ident| $body:expr $(,)?) => {
        $crate::rate_limited!(
            $crate::utils::log_rate_limit::LOG_RATE_LIMIT_INTERVAL,
            |$suppressed| $body
        )
    };
    ($interval:expr, |$suppressed:ident| $body:expr $(,)?) => {{
        static LIMITER: $crate::utils::log_rate_limit::LogRateLimiter =
            $crate::utils::log_rate_limit::LogRateLimiter::new();
        if let ::core::option::Option::Some($suppressed) = LIMITER.check($interval) {
            $body;
        }
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: Duration = Duration::from_secs(1);

    #[test]
    fn admits_one_event_per_interval_and_counts_the_others() {
        let limiter = LogRateLimiter::new();
        let ms = Duration::from_millis;

        assert_eq!(limiter.check_at(ms(0), SECOND), Some(0));
        assert_eq!(limiter.check_at(ms(1), SECOND), None);
        assert_eq!(limiter.check_at(ms(999), SECOND), None);
        // the next admitted event reports the suppressed ones
        assert_eq!(limiter.check_at(ms(1000), SECOND), Some(2));
        assert_eq!(limiter.check_at(ms(1500), SECOND), None);
        // after a quiet period an event is admitted right away
        assert_eq!(limiter.check_at(ms(5000), SECOND), Some(1));
        assert_eq!(limiter.check_at(ms(5001), SECOND), None);
    }

    #[test]
    fn macro_skips_the_expression_while_suppressed() {
        let mut evaluated = Vec::new();
        for _ in 0..100 {
            crate::rate_limited!(Duration::from_secs(3600), |suppressed| {
                evaluated.push(suppressed)
            });
        }
        // the first evaluation within the hour, the others counted for the next one
        assert_eq!(evaluated, vec![0]);
    }

    #[test]
    fn call_sites_are_limited_independently() {
        let mut first = 0;
        let mut second = 0;
        for _ in 0..10 {
            crate::rate_limited!(Duration::from_secs(3600), |_s| first += 1);
            crate::rate_limited!(Duration::from_secs(3600), |_s| second += 1);
        }
        assert_eq!((first, second), (1, 1));
    }
}
