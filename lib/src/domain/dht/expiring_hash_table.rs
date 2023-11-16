use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::time::Instant;

use crate::domain::dht::DEFAULT_SLICE_SIZE;
use crate::domain::dht::{TimeoutStrategy, Expiring, HashTable, DHTData};
use crate::messaging::dht_messaging::{StoreOK, StoreErr, FetchErr};

struct TimedValue<V> {
    pub value: V,
    pub time: Instant,
}

// only derives hash and eq from value not time
impl<V> TimedValue<V> {
    fn new(value: V) -> Self {
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

enum TimedHTData<T, const SLICE_SIZE: usize = DEFAULT_SLICE_SIZE> {
    Single(TimedValue<T>),
    Slice(TimedValue<[T; SLICE_SIZE]>),
    Set(HashSet<TimedValue<T>>),
}

pub struct ExpiringHashTable<H, D> {
    map: HashMap<H, D>,
}

impl<H, T> Expiring<H> for ExpiringHashTable<H, TimedHTData<T>>
    where
        H: Eq + Hash,
        T: Eq + Hash
{
    fn expire(&mut self) {
        self.map.clear() // maybe we should not keep the memory?
    }

    fn expire_with_strategy(&mut self, strategy: &impl TimeoutStrategy<H>) {
        for (handle, data) in self.map.iter_mut() { // needs to be mutable for list edit
            match data {
                TimedHTData::Single(tv) => {
                    if strategy.is_timed_out(handle, &tv.time) {
                        self.map.remove(handle);
                    }
                }
                TimedHTData::Slice(tv) => {
                    if strategy.is_timed_out(handle, &tv.time) {
                        self.map.remove(handle);
                    }
                }
                TimedHTData::Set(set) => {
                    for tv in set.iter() {
                        if strategy.is_timed_out(handle, &tv.time) {
                            set.remove(tv);
                        }
                    }
                }
            }
        }
    }
}

impl<H, T> HashTable<H, DHTData<T>> for ExpiringHashTable<H, TimedHTData<T>>
    where H: Eq + Hash, T: Eq + Hash
{
    type StoreErr = StoreErr;
    type StoreOK = StoreOK;
    type FetchErr = FetchErr;

    fn store(&mut self, handle: H, data: DHTData<T>) -> Result<Self::StoreOK, Self::StoreErr> {
        match self.map.get_mut(&handle) {
            Some(TimedHTData::Set(set)) => {
                if let DHTData::Set(s) = data {
                    return if let Some(_) = set.replace(TimedValue::new(s)) {
                        Ok(StoreOK::Updated)
                    } else {
                        Ok(StoreOK::Created)
                    };
                }
            }
            Some(TimedHTData::Single(..)) => {
                if let DHTData::Single(s) = data {
                    self.map.insert(handle, TimedHTData::Single(TimedValue::new(s)));
                    return Ok(StoreOK::Updated);
                }
            }
            Some(TimedHTData::Slice(..)) => {
                if let DHTData::Slice(s) = data {
                    self.map.insert(handle, TimedHTData::Single(TimedValue::new(s)));
                    return Ok(StoreOK::Updated);
                }
            }
            None => {
                match data {
                    DHTData::Single(s) => self.map.insert(handle, TimedHTData::Single(TimedValue::new(s))),
                    DHTData::Slice(s) => self.map.insert(handle, TimedHTData::Slice(TimedValue::new(s))),
                    DHTData::Set(lv) => {
                        let mut set = HashSet::default();
                        set.insert(TimedValue::new(lv));
                        self.map.insert(handle, TimedHTData::Set(set))
                    }
                }
                return Ok(StoreOK::Created);
            }
        }

        Err(StoreErr::DataTypeErr)
    }
    fn fetch(&self, handle: &H) -> Result<DHTData<T>, Self::FetchErr> {
        // todo soll ich hier überprüfen ob die Daten expired sind?

        match self.map.get(handle) {
            None => Err(FetchErr::NotFoundErr),
            Some(TimedHTData::Single(ts)) => Ok(DHTData::Single(ts.value.clone())),
            Some(TimedHTData::Slice(ts)) => Ok(DHTData::Slice(ts.value.clone())),
            Some(TimedHTData::Set(vec)) => todo!() // todo was soll hier zurückgegeben werden
        }
    }
}

impl<H, D> Default for ExpiringHashTable<H, D> {
    fn default() -> Self {
        Self {
            map: HashMap::default()
        }
    }
}