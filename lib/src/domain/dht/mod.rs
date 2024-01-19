use std::hash::{Hash, Hasher};
use std::time::Instant;

pub use expiring::Expiring;

pub mod hash_table;
pub mod strategies;
mod expiring;

/// A generic struct representing a value with an associated timestamp.
///
/// # Fields
///
/// - `value`: The value of type `V`.
/// - `time`: The timestamp of type `Instant`.
///
/// The `value` field can hold any value of any type, while the `time` field holds the timestamp
/// when the value was recorded.
///
/// ## [Eq] and [Hash]
///
/// It is important to note that this struct only derives [Eq] and [Hash] from the `value`,
/// thereby restricting the insertion of a `value` multiple times in a [HashSet](std::collections::HashSet)
/// with only a differing `time`.
#[derive(Debug, Clone)]
pub struct TimedValue<V> {
    pub value: V,
    pub timestamp: Instant,
}

impl<V> TimedValue<V> {
    /// Creates a new instance of `Self` with the specified `value`.
    ///
    /// # Arguments
    ///
    /// * `value` - The initial value for the instance.
    ///
    /// # Returns
    ///
    /// The newly created instance of `Self` with the current time ([Instant::now()]).
    pub fn new(value: V) -> Self {
        Self {
            value,
            timestamp: Instant::now(),
        }
    }

    /// Updates the time for the current instance.
    ///
    /// This function updates the time for the current instance by assigning it a new value obtained from [Instant::now].
    pub fn update(&mut self) {
        self.timestamp = Instant::now();
    }
}

impl<V: Default> Default for TimedValue<V> {
    fn default() -> Self {
        Self {
            value: V::default(),
            timestamp: Instant::now(),
        }
    }
}

impl<V> From<V> for TimedValue<V> {
    fn from(value: V) -> Self {
        Self::new(value)
    }
}

impl<V: Eq> PartialEq<Self> for TimedValue<V> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<V: Eq> Eq for TimedValue<V> {}

impl<V: Hash> Hash for TimedValue<V> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.hash(state)
    }
}