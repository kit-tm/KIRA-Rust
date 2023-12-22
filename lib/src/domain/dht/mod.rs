use std::hash::{Hash, Hasher};
use std::time::Instant;
use serde::Serialize;

pub mod hash_table;
pub mod strategies;
pub mod expiring;

#[derive(Debug, Clone, Serialize)]
pub struct TimedValue<V> {
    pub value: V,
    pub time: Instant,
}

// only derives hash and eq from value not time
impl<V> TimedValue<V> {
    pub fn new(value: V) -> Self {
        Self {
            value,
            time: Instant::now(),
        }
    }

    pub fn update(&mut self) {
        self.time = Instant::now();
    }
}

impl<V: Eq> PartialEq<Self> for TimedValue<V> {
    fn eq(&self, other: &Self) -> bool {
        self.time == other.time
    }
}

impl<V: Eq> Eq for TimedValue<V> {}

impl<V: Hash> Hash for TimedValue<V> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.time.hash(state)
    }
}