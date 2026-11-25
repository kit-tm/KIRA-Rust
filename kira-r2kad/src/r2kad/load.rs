//! Estimates of how loaded the node and the network are, to adapt the timeouts of requests.
//!
//! With fixed timeouts an overloaded network collapses: responses arrive (or are processed)
//! after the timeouts of their requests, the requests are sent again, which adds load, so
//! more responses come too late. This happens, e.g., when all nodes start at once after a
//! power outage.
//!
//! The timeout of a request therefore grows with
//! - the round-trip times of recent requests (smoothed as in RFC 6298), which include the
//!   time remote nodes take to process them,
//! - the time received messages wait before this node processes them (its own backlog),
//! - a backoff doubling with every request that timed out until a response arrives again.
//!
//! Under load a node also sheds work:
//! - it drops lookups that waited so long before it could process them that their senders
//!   have given up on them ([LoadEstimator::stale_request_age]),
//! - it defers periodic work that can wait, e.g., the random exploration of the overlay
//!   ([LoadEstimator::load_factor]).

use std::time::Duration;

/// Upper bound of the timeout of a request, unless its base timeout is larger.
pub const MAX_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Upper bound of the backoff factor of request timeouts.
pub const MAX_BACKOFF: u32 = 8;
/// Queueing delay per step of the [load factor](LoadEstimator::load_factor).
pub const LOAD_FACTOR_STEP: Duration = Duration::from_millis(100);
/// Upper bound of the [load factor](LoadEstimator::load_factor).
pub const MAX_LOAD_FACTOR: u32 = 8;
/// Base timeout of the requests whose [stale age](LoadEstimator::stale_request_age) is used,
/// the shortest common one (e.g., of FindNodeReqs).
pub const STALE_REQUEST_BASE_TIMEOUT: Duration = Duration::from_millis(250);

/// See the [module documentation](self).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadEstimator {
    queueing_delay: Duration,
    srtt: Option<Duration>,
    rttvar: Duration,
    backoff: u32,
}

impl Default for LoadEstimator {
    fn default() -> Self {
        Self {
            queueing_delay: Duration::ZERO,
            srtt: None,
            rttvar: Duration::ZERO,
            backoff: 1,
        }
    }
}

impl LoadEstimator {
    /// Records how long a received message waited before it was processed.
    ///
    /// The estimate rises quickly (by half the difference), so a backlog is noticed within a
    /// few messages, and falls slowly (by an eighth).
    pub fn record_queueing_delay(&mut self, delay: Duration) {
        let estimate = self.queueing_delay;
        self.queueing_delay = if delay > estimate {
            estimate + (delay - estimate) / 2
        } else {
            estimate - (estimate - delay) / 8
        };
    }

    /// Smoothed time received messages wait before they are processed.
    pub fn queueing_delay(&self) -> Duration {
        self.queueing_delay
    }

    /// Records the round-trip time of a request and its response (RFC 6298), which also
    /// ends the backoff.
    pub fn record_rtt(&mut self, rtt: Duration) {
        match self.srtt {
            None => {
                self.srtt = Some(rtt);
                self.rttvar = rtt / 2;
            }
            Some(srtt) => {
                let deviation = srtt.abs_diff(rtt);
                self.rttvar = self.rttvar - self.rttvar / 4 + deviation / 4;
                self.srtt = Some(srtt - srtt / 8 + rtt / 8);
            }
        }
        self.backoff = 1;
    }

    /// Records that a request got no response in time: doubles the backoff.
    pub fn record_request_timeout(&mut self) {
        self.backoff = (self.backoff * 2).min(MAX_BACKOFF);
    }

    /// Timeout of a request with the configured `base` timeout.
    ///
    /// It is `max(base, SRTT + 4 * RTTVAR)` times the backoff, at most
    /// `max(base, MAX_REQUEST_TIMEOUT)`, plus the queueing delay of received messages. In an
    /// idle network it equals `base`.
    pub fn request_timeout(&self, base: Duration) -> Duration {
        let rto = self
            .srtt
            .map_or(Duration::ZERO, |srtt| srtt + 4 * self.rttvar);
        let timeout = base.max(rto) * self.backoff;
        timeout.min(base.max(MAX_REQUEST_TIMEOUT)) + self.queueing_delay
    }

    /// Factor to stretch the intervals of periodic work that can wait: one more per
    /// [LOAD_FACTOR_STEP] of queueing delay, at most [MAX_LOAD_FACTOR]. 1 without backlog.
    pub fn load_factor(&self) -> u32 {
        let steps = self.queueing_delay.as_nanos() / LOAD_FACTOR_STEP.as_nanos();
        u32::try_from(steps)
            .unwrap_or(u32::MAX)
            .saturating_add(1)
            .min(MAX_LOAD_FACTOR)
    }

    /// Time after which a received lookup isn't worth processing anymore: its sender, waiting
    /// for the response with a timeout adapted to similar load, has given up on it.
    ///
    /// Twice the request timeout of [STALE_REQUEST_BASE_TIMEOUT], so at least 500ms.
    pub fn stale_request_age(&self) -> Duration {
        2 * self.request_timeout(STALE_REQUEST_BASE_TIMEOUT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn idle_network_keeps_base_timeout() {
        let mut load = LoadEstimator::default();
        assert_eq!(load.request_timeout(ms(250)), ms(250));
        // fast round trips don't shorten it
        for _ in 0..20 {
            load.record_rtt(ms(2));
            load.record_queueing_delay(Duration::ZERO);
        }
        assert_eq!(load.request_timeout(ms(250)), ms(250));
    }

    #[test]
    fn slow_round_trips_lengthen_the_timeout() {
        let mut load = LoadEstimator::default();
        load.record_rtt(ms(400));
        // SRTT 400ms, RTTVAR 200ms
        assert_eq!(load.request_timeout(ms(250)), ms(1200));
        // a larger base timeout still applies
        assert_eq!(load.request_timeout(ms(3000)), ms(3000));
    }

    #[test]
    fn queueing_delay_rises_fast_and_falls_slowly() {
        let mut load = LoadEstimator::default();
        load.record_queueing_delay(ms(800));
        assert_eq!(load.queueing_delay(), ms(400));
        load.record_queueing_delay(ms(800));
        assert_eq!(load.queueing_delay(), ms(600));
        load.record_queueing_delay(Duration::ZERO);
        assert_eq!(load.queueing_delay(), ms(525));
        // the backlog adds to the timeout
        assert_eq!(load.request_timeout(ms(250)), ms(775));
    }

    #[test]
    fn timeouts_back_off_until_a_response_arrives() {
        let mut load = LoadEstimator::default();
        for expected in [500, 1000, 2000, 2000] {
            load.record_request_timeout();
            assert_eq!(load.request_timeout(ms(250)), ms(expected));
        }
        load.record_rtt(ms(10));
        assert_eq!(load.request_timeout(ms(250)), ms(250));
    }

    #[test]
    fn load_factor_grows_with_the_queueing_delay() {
        let mut load = LoadEstimator::default();
        assert_eq!(load.load_factor(), 1);
        load.record_queueing_delay(ms(500)); // smoothed 250ms
        assert_eq!(load.load_factor(), 3);
        for _ in 0..10 {
            load.record_queueing_delay(Duration::from_secs(10));
        }
        assert_eq!(load.load_factor(), MAX_LOAD_FACTOR);
    }

    #[test]
    fn stale_request_age_follows_the_request_timeout() {
        let mut load = LoadEstimator::default();
        assert_eq!(load.stale_request_age(), ms(500));
        load.record_queueing_delay(Duration::from_secs(2)); // smoothed 1s
        assert_eq!(load.stale_request_age(), ms(2500));
    }

    #[test]
    fn timeout_is_bounded() {
        let mut load = LoadEstimator::default();
        load.record_rtt(Duration::from_secs(30));
        for _ in 0..5 {
            load.record_request_timeout();
        }
        assert_eq!(load.request_timeout(ms(250)), MAX_REQUEST_TIMEOUT);
        assert_eq!(
            load.request_timeout(Duration::from_secs(20)),
            Duration::from_secs(20)
        );
    }
}
