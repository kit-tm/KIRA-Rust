use std::collections::HashSet;
use std::fmt::Debug;
use std::hash::{Hash, Hasher};
use std::time::Instant;
use serde::{Serialize};
use serde::de::DeserializeOwned;

use crate::domain::dht::{ContextExpiring, TimeoutStrategy};
use crate::domain::dht::hash_table::{FetchStrategy, InsertStrategy};
use crate::messaging::dht::{FetchErr, StoreErr, StoreOK};

#[derive(Debug)]
pub enum DHTInput<T: Debug + DeserializeOwned + Serialize> {
    Single(T),
    Set(T),
}

impl<T> InsertStrategy<TimedHTData<T>> for DHTInput<T>
    where T: Debug + DeserializeOwned + Serialize
{
    type InsertOk = StoreOK;
    type InsertErr = StoreErr;

    fn insert(self, into: &mut TimedHTData<T>) -> Result<Self::InsertOk, Self::InsertErr> {
        match into {
            TimedHTData::Single(mut single) => {
                if let Self::Single(value) = self {
                    single = TimedValue::new(value);
                    return Ok(StoreOK::Updated);
                }
            }
            TimedHTData::Set(mut set) => {
                if let Self::Set(value) = self {
                    return if let Some(_) = set.replace(TimedValue::new(value)) {
                        Ok(StoreOK::Updated)
                    } else {
                        Ok(StoreOK::Created)
                    };
                }
            }
        }

        return Err(StoreErr::DataTypeErr);
    }
}

impl<T> Into<TimedHTData<T>> for DHTInput<T> {
    fn into(self) -> TimedHTData<T> {
        match self {
            DHTInput::Single(value) => { TimedHTData::Single(TimedValue::new(value)) }
            DHTInput::Set(value) => {
                let mut set = HashSet::default();
                set.insert(TimedValue::new(value));
                set
            }
        }
    }
}

pub enum DHTOutput<T: Debug + DeserializeOwned + Serialize> {
    Single(T),
    Set(Vec<T>),
}

impl<T> FetchStrategy<TimedHTData<T>> for DHTOutput<T> {
    type FetchErr = FetchErr;

    fn fetch(from: &TimedHTData<T>) -> Result<&Self, Self::FetchErr> {
        match from {
            TimedHTData::Single(timed_single) => { Self::Single(&timed_single.value) }
            TimedHTData::Set(set) => {
                let vec: Vec<T> = set.into_iter().collect();
                Self::Set(vec)
            }
        }

        Err(FetchErr::NotFoundErr)
    }
}

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

pub enum TimedHTData<T> {
    Single(TimedValue<T>),
    Set(HashSet<TimedValue<T>>),
}

impl<C, T> ContextExpiring<C> for TimedHTData<T> {
    fn collect_with_context(&mut self, context: &C, strategy: &impl TimeoutStrategy<C>) -> bool {
        match self {
            Self::Single(tv) => {
                strategy.is_timed_out(context, &tv.time)
            }
            Self::Set(set) => {
                for tv in set.iter() {
                    if strategy.is_timed_out(context, &tv.time) {
                        set.remove(tv);
                    }
                }
                set.is_empty()
            }
        }
    }
}