use std::time::Duration;

use derive_more::derive::Display;
use rand::random_range;

/// An exponential Backoff strategy based on a [Duration] and maximum number of retries.
#[derive(Debug, Clone, Eq, PartialEq, Copy, Display)]
#[display("Backoff {current_retries}/{max_retries}")]
pub struct ExponentialBackoff {
    pub base: u32,
    pub max_retries: u32,
    pub current_retries: u32,
    pub start_duration: Duration,
    randomize: bool, // add random offset [-0.5,+0.5]*duration (resolution up to µs)
    limit: bool, // if set next() will never return none and stay at the max_retries value forever (until reset)
}

impl ExponentialBackoff {
    /// Creates a new [ExponentialBackoff] with default base **2**.
    pub fn with_default_base(max_retries: u32, start_duration: Duration) -> Self {
        Self::new(2, max_retries, start_duration)
    }

    /// Creates a new [ExponentialBackoff] with default base **2**.
    pub fn random_with_default_base(max_retries: u32, start_duration: Duration) -> Self {
        Self::new_random(2, max_retries, start_duration)
    }

    /// Creates a new [ExponentialBackoff].
    pub fn new(base: u32, max_retries: u32, start_duration: Duration) -> Self {
        Self {
            base,
            max_retries,
            current_retries: 0,
            start_duration,
            randomize: false,
            limit: false,
        }
    }

    /// Creates a new [ExponentialBackoff] with randomized offsets
    pub fn new_random(base: u32, max_retries: u32, start_duration: Duration) -> Self {
        Self {
            base,
            max_retries,
            current_retries: 0,
            start_duration,
            randomize: true,
            limit: false,
        }
    }

    /// Resets the exponential backoff by setting the number of already performed
    /// tries to 0.
    pub fn reset(&mut self) {
        self.current_retries = 0;
    }

    /// if max_retries is reached, the duration will stay at this limit forever
    pub fn set_limit(&mut self) {
        self.limit = true;
    }

    /// if max_retries is reached, next() will return None
    pub fn clear_limit(&mut self) {
        self.limit = false;
    }

    /// Returns if the backoff was started.
    ///
    /// This means the method [Iterator::next] was called at least once and the backoff
    /// was not reset since then.
    pub fn is_started(&self) -> bool {
        self.current_retries != 0
    }
}

impl Iterator for ExponentialBackoff {
    type Item = Duration;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current_retries >= self.max_retries {
            return None;
        }

        let mut duration = self.base.pow(self.current_retries) * self.start_duration;
        if self.randomize {
            let lower_bound = duration.as_micros() / 2;
            let upper_bound = 3 * duration.as_micros() / 2;
            duration = Duration::from_micros(random_range(lower_bound..upper_bound) as u64);
        }
        self.current_retries += 1;
        // if limit is set, we stay with self.current_retries at self.max_retries - 1
        if self.limit && self.current_retries == self.max_retries && self.max_retries > 0 {
            self.current_retries -= 1;
        }

        Some(duration)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::utils::ExponentialBackoff;

    #[test]
    fn iter_emits_max_retries_times() {
        let mut backoff = ExponentialBackoff::with_default_base(5, Duration::from_micros(5));
        assert!(backoff.next().is_some());
        assert!(backoff.next().is_some());
        assert!(backoff.next().is_some());
        assert!(backoff.next().is_some());
        assert!(backoff.next().is_some());
        assert!(backoff.next().is_none());
    }

    #[test]
    fn iter_emits_max_retries_times_correct_value() {
        let mut backoff = ExponentialBackoff::with_default_base(5, Duration::from_micros(1));
        assert_eq!(backoff.next(), Some(Duration::from_micros(1)));
        assert_eq!(backoff.next(), Some(Duration::from_micros(2)));
        assert_eq!(backoff.next(), Some(Duration::from_micros(4)));
        assert_eq!(backoff.next(), Some(Duration::from_micros(8)));
        assert_eq!(backoff.next(), Some(Duration::from_micros(16)));
        assert_eq!(backoff.next(), None);
    }

    #[test]
    fn iter_works_with_limit_correctly() {
        let mut backoff = ExponentialBackoff::with_default_base(5, Duration::from_micros(1));
        backoff.set_limit();
        assert_eq!(backoff.next(), Some(Duration::from_micros(1)));
        assert_eq!(backoff.next(), Some(Duration::from_micros(2)));
        assert_eq!(backoff.next(), Some(Duration::from_micros(4)));
        assert_eq!(backoff.next(), Some(Duration::from_micros(8)));
        assert_eq!(backoff.next(), Some(Duration::from_micros(16)));
        assert_eq!(backoff.next(), Some(Duration::from_micros(16)));
        assert_eq!(backoff.next(), Some(Duration::from_micros(16)));
        assert_eq!(backoff.next(), Some(Duration::from_micros(16)));
        assert_eq!(backoff.next(), Some(Duration::from_micros(16)));
        backoff.clear_limit();
        assert_eq!(backoff.next(), Some(Duration::from_micros(16)));
        assert_eq!(backoff.next(), None);
    }

    #[test]
    fn random_provides_correct_value() {
        let mut backoff =
            ExponentialBackoff::random_with_default_base(5, Duration::from_millis(100));
        let next_value = backoff.next(); // 100
        assert!(
            Duration::from_millis(50) <= next_value.unwrap()
                && next_value.unwrap() <= Duration::from_millis(150)
        );
        let next_value = backoff.next(); // 200
        assert!(
            Duration::from_millis(100) <= next_value.unwrap()
                && next_value.unwrap() <= Duration::from_millis(300)
        );
        let next_value = backoff.next(); // 400
        assert!(
            Duration::from_millis(200) <= next_value.unwrap()
                && next_value.unwrap() <= Duration::from_millis(600)
        );
        let next_value = backoff.next(); // 800
        assert!(
            Duration::from_millis(400) <= next_value.unwrap()
                && next_value.unwrap() <= Duration::from_millis(1200)
        );
        let next_value = backoff.next(); // 1600
        assert!(
            Duration::from_millis(800) <= next_value.unwrap()
                && next_value.unwrap() <= Duration::from_millis(2400)
        );
        assert_eq!(backoff.next(), None);
    }
}
