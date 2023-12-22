use std::hash::{Hash, Hasher};
use std::time::Instant;
use serde::Serialize;

pub mod hash_table;
pub mod strategies;
pub mod expiring;

#[derive(Debug, Clone, Serialize)]
pub struct TimedValue<V> {
    pub value: V,
    #[serde(with = "approx_instant")]
    pub time: Instant,
}

mod approx_instant {
    // https://github.com/serde-rs/serde/issues/1375#issuecomment-419688068
    use std::time::{Instant, SystemTime};
    use chrono::{Local, DateTime};
    use serde::{Serialize, Serializer};

    pub fn serialize<S>(instant: &Instant, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
    {
        let system_now = SystemTime::now();
        let instant_now = Instant::now();
        let approx = system_now - (instant_now - *instant);
        let now: DateTime<Local> = approx.into();
        now.to_rfc3339().serialize(serializer)
    }
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