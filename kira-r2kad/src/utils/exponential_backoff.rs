use std::time::Duration;

use derive_more::derive::Display;

/// An exponential Backoff strategy based on a [Duration] and maximum number of retries.
#[derive(Debug, Clone, Eq, PartialEq, Display)]
#[display("Backoff {current_retries}/{max_retries}")]
pub struct ExponentialBackoff {
    pub base: u32,
    pub max_retries: u32,
    pub current_retries: u32,
    pub start_duration: Duration,
}

impl ExponentialBackoff {
    /// Creates a new [ExponentialBackoff] with default base **2**.
    pub fn with_default_base(max_retries: u32, start_duration: Duration) -> Self {
        Self::new(2, max_retries, start_duration)
    }

    /// Creates a new [ExponentialBackoff].
    pub fn new(base: u32, max_retries: u32, start_duration: Duration) -> Self {
        Self {
            base,
            max_retries,
            current_retries: 0,
            start_duration,
        }
    }

    /// Resets the exponential backoff by setting the number of already performed
    /// tries to 0.
    pub fn reset(&mut self) {
        self.current_retries = 0;
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

        let duration = self.base.pow(self.current_retries) * self.start_duration;
        self.current_retries += 1;

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
}
