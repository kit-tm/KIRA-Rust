use std::{
    collections::{
        HashMap, HashSet,
        hash_map::Entry::{Occupied, Vacant},
    },
    sync::Arc,
    time::Instant,
};

use super::EntryMeta;
use super::LocalHashTable;

use crate::{
    domain::{DEFAULT_BUCKET_SIZE, NodeId},
    messaging::dht,
};

use derive_more::{Display, Error};

/// A hash table which can hold multiple value per key.
///
/// Currently this implementation doesn't differ much from the [SingleValueHashTable].
///
/// [SingleValueHashTable]: super::SingleValueHashTable
#[derive(Debug, Default, Clone)]
pub struct ComplexHashTable<const BUCKET_SIZE: usize = DEFAULT_BUCKET_SIZE> {
    inner: HashMap<NodeId, Entry>,
}

impl LocalHashTable for ComplexHashTable {
    type StoreOk = StoreOk;
    type StoreErr = StoreErr;
    type FetchErr = FetchErr;

    fn store(&mut self, key: NodeId, value: Arc<[u8]>) -> Result<Self::StoreOk, Self::StoreErr> {
        Ok(match self.inner.entry(key) {
            Occupied(occ) => {
                let new_value = occ.into_mut().values.insert(value);
                if new_value {
                    StoreOk::EntryAppended(key)
                } else {
                    StoreOk::EntryUnmodified(key)
                }
            }
            Vacant(vac) => {
                vac.insert(Entry::new(value));
                StoreOk::NewEntry(key)
            }
        })
    }

    fn fetch(&self, key: &NodeId) -> Result<impl Iterator<Item = Arc<[u8]>>, Self::FetchErr> {
        let Some(entry) = self.inner.get(key) else {
            return Err(FetchErr::EntryNotFound(*key));
        };
        Ok(entry.values.iter().cloned())
    }

    fn fetch_all(&self) -> impl Iterator<Item = (&NodeId, impl Iterator<Item = Arc<[u8]>>)> {
        self.inner
            .iter()
            .map(|(k, e)| (k, e.values.iter().cloned()))
    }

    fn remove(&mut self, key: &NodeId) -> bool {
        self.inner.remove(key).is_some()
    }

    fn meta(&self, key: &NodeId) -> Option<&impl super::EntryMeta> {
        self.inner.get(key)
    }

    fn meta_mut(&mut self, key: &NodeId) -> Option<&mut impl super::EntryMeta> {
        self.inner.get_mut(key)
    }
}

#[derive(Debug, Display)]
pub enum StoreOk {
    #[display("New entry created: {_0}")]
    NewEntry(NodeId),
    #[display("Value already present in entry: {_0}")]
    EntryUnmodified(NodeId),
    #[display("Append an existing entry with a new value: {_0}")]
    EntryAppended(NodeId),
}

impl From<StoreOk> for dht::StoreOk {
    fn from(value: StoreOk) -> Self {
        match value {
            StoreOk::NewEntry(_) => dht::StoreOk::Created,
            StoreOk::EntryUnmodified(_) => dht::StoreOk::Updated,
            StoreOk::EntryAppended(_) => dht::StoreOk::Inserted,
        }
    }
}

// currently no errors possible, future errors could be:
//   - max number of values reached
//   - permissions
#[derive(Debug, Display, Error)]
pub enum StoreErr {}

impl From<StoreErr> for dht::StoreErr {
    fn from(value: StoreErr) -> Self {
        match value {}
    }
}

#[derive(Debug, Display, Error)]
pub enum FetchErr {
    #[display("Entry not found: {_0}")]
    EntryNotFound(#[error(ignore)] NodeId),
}

impl From<FetchErr> for dht::FetchErr {
    fn from(value: FetchErr) -> Self {
        match value {
            FetchErr::EntryNotFound(_) => dht::FetchErr::NotFoundErr,
        }
    }
}

#[derive(Debug, Clone)]
struct Entry {
    // data
    values: HashSet<Arc<[u8]>>,

    // metadata
    accessed: Option<Instant>,
    republished: Option<Instant>,
}

impl Entry {
    fn new(value: Arc<[u8]>) -> Self {
        Self {
            values: HashSet::from([value]),
            accessed: None,
            republished: None,
        }
    }
}

impl EntryMeta for Entry {
    fn last_access(&self) -> Option<Instant> {
        self.accessed
    }

    fn access(&mut self, now: Instant) -> bool {
        // prohibit updates of the access into the past
        if self
            .accessed
            .is_none_or(|last_accessed| last_accessed < now)
        {
            self.accessed.replace(now);
            true
        } else {
            false
        }
    }

    fn last_republish(&self) -> Option<Instant> {
        self.republished
    }

    fn republished(&mut self, now: Instant) -> bool {
        // prohibit updates of the republish time into the past
        if self
            .republished
            .is_none_or(|last_republished| last_republished < now)
        {
            self.republished.replace(now);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn test_store_and_fetch_new_entry() {
        let mut table = ComplexHashTable::default();
        let key = NodeId::with_msb(42);
        let value = vec![1, 2, 3];

        let store_result = table.store(key, value.clone().into());
        assert!(
            matches!(store_result, Ok(StoreOk::NewEntry(k)) if k == key),
            "Expected NewEntry, got {:?}",
            store_result
        );

        let fetch_result = table.fetch(&key).expect("Failed to fetch entry");
        let fetched_values: Vec<_> = fetch_result.into_iter().collect();

        assert_eq!(fetched_values.len(), 1);
        assert_eq!(&*fetched_values[0], value.as_slice());
    }

    #[test]
    fn test_store_updates_existing_entry() {
        let mut table = ComplexHashTable::default();
        let key = NodeId::with_msb(42);
        let val = vec![1, 1, 1];

        table.store(key, val.clone().into()).unwrap();

        let update_result = table.store(key, val.clone().into());
        assert!(
            matches!(update_result, Ok(StoreOk::EntryUnmodified(k)) if k == key),
            "Expected EntryUnmodified, got {:?}",
            update_result
        );

        let fetch_result = table.fetch(&key).unwrap();
        let fetched_values: Vec<_> = fetch_result.into_iter().collect();
        assert_eq!(fetched_values.len(), 1);
        assert_eq!(*fetched_values[0], val);
    }

    #[test]
    fn test_store_append_entry() {
        let mut table = ComplexHashTable::default();
        let key = NodeId::with_msb(42);
        let val1 = vec![1, 1, 1];
        let val2 = vec![2, 2, 2];

        table.store(key, val1.clone().into()).unwrap();

        let update_result = table.store(key, val2.clone().into());
        assert!(
            matches!(update_result, Ok(StoreOk::EntryAppended(k)) if k == key),
            "Expected EntryAppended, got {:?}",
            update_result
        );

        let fetch_result = table.fetch(&key).unwrap();
        let fetched_values: Vec<_> = fetch_result.into_iter().collect();
        assert_eq!(fetched_values.len(), 2);
        assert_eq!(*fetched_values[0], val1);
        assert_eq!(*fetched_values[1], val2);
    }

    #[test]
    fn test_fetch_missing_entry_returns_error() {
        let table = ComplexHashTable::default();
        let key = NodeId::with_msb(42);

        let fetch_result = table.fetch(&key);
        assert!(
            matches!(fetch_result, Err(FetchErr::EntryNotFound(k)) if k == key),
            "Expected EntryNotFound error",
        );
    }

    #[test]
    fn test_remove() {
        let mut table = ComplexHashTable::default();
        let key = NodeId::with_msb(42);
        let val = vec![1, 1, 1];

        table.store(key, val.into()).unwrap();

        assert!(table.remove(&key));
        assert!(matches!(table.fetch(&key), Err(FetchErr::EntryNotFound(k)) if k == key));
    }

    #[test]
    fn test_remove_empty() {
        let mut table = ComplexHashTable::default();
        let key = NodeId::with_msb(42);

        assert!(!table.remove(&key));
    }

    #[test]
    fn test_fetch_all_empty() {
        let table = ComplexHashTable::default();
        let all_entries: Vec<_> = table.fetch_all().collect();
        assert!(all_entries.is_empty());
    }

    #[test]
    fn test_fetch_all_populated() {
        let mut table = ComplexHashTable::default();
        let key1 = NodeId::with_msb(42);
        let key2 = NodeId::with_msb(69);
        let val1: Arc<[u8]> = Arc::new([1, 2, 3]);
        let val2: Arc<[u8]> = Arc::new([4, 5, 6]);
        let val3: Arc<[u8]> = Arc::new([7, 8, 9]);

        table.store(key1, val1.clone()).unwrap();
        table.store(key1, val2.clone()).unwrap();
        table.store(key2, val3.clone()).unwrap();

        let results: HashMap<NodeId, HashSet<Arc<[u8]>>> = table
            .fetch_all()
            .map(|(k, values)| (*k, values.collect()))
            .collect();

        assert_eq!(results.get(&key1).unwrap().len(), 2);
        assert!(results.get(&key1).unwrap().contains(&val1));
        assert!(results.get(&key1).unwrap().contains(&val2));

        assert_eq!(results.get(&key2).unwrap().len(), 1);
        assert!(results.get(&key2).unwrap().contains(&val3));
    }

    #[test]
    fn test_entry_meta_access_updates() {
        let mut entry = Entry::new(Arc::new([0, 0, 0]));

        assert_eq!(
            entry.last_access(),
            None,
            "Entry initially shouldn't have been accessed"
        );

        let inital_access_time = Instant::now();
        let second_access_time = inital_access_time + Duration::from_secs(10);
        let time_past = inital_access_time - Duration::from_secs(10);

        assert!(
            entry.access(inital_access_time),
            "Failed to record initial access"
        );
        assert_eq!(entry.last_access(), Some(inital_access_time));

        assert!(
            entry.access(second_access_time),
            "Failed to record repeated access"
        );
        assert_eq!(entry.last_access(), Some(second_access_time));

        assert!(
            !entry.access(time_past),
            "Recording access times from the past is prohibited"
        );
        assert_eq!(
            entry.last_access(),
            Some(second_access_time),
            "Access time was mutated by a past access request"
        );
    }

    #[test]
    fn test_table_meta_and_meta_mut() {
        let mut table = ComplexHashTable::default();
        let key = NodeId::with_msb(42);
        table.store(key, Arc::new([10, 20, 30])).unwrap();

        let now = Instant::now();

        {
            let meta_mut = table.meta_mut(&key).expect("Entry should exist");
            assert!(meta_mut.access(now));
            assert!(meta_mut.republished(now));
        }
        {
            let meta = table.meta(&key).expect("Entry should exist");
            assert_eq!(meta.last_access(), Some(now));
            assert_eq!(meta.last_republish(), Some(now));
        }

        // Test meta for non-existent key
        let missing_key = NodeId::with_msb(69);
        assert!(table.meta(&missing_key).is_none());
        assert!(table.meta_mut(&missing_key).is_none());
    }

    #[test]
    fn test_entry_meta_republish_updates() {
        let mut entry = Entry::new(Arc::new([0, 0, 0]));

        assert_eq!(
            entry.last_republish(),
            None,
            "Entry initially shouldn't have been republished"
        );

        let inital_republish_time = Instant::now();
        let second_republish_time = inital_republish_time + Duration::from_secs(10);
        let time_past = inital_republish_time - Duration::from_secs(10);

        assert!(
            entry.republished(inital_republish_time),
            "Failed to record initial republish"
        );
        assert_eq!(entry.last_republish(), Some(inital_republish_time));

        assert!(
            entry.republished(second_republish_time),
            "Failed to record repeated republish"
        );
        assert_eq!(entry.last_republish(), Some(second_republish_time));

        assert!(
            !entry.republished(time_past),
            "Recording republish times from the past is prohibited"
        );
        assert_eq!(
            entry.last_republish(),
            Some(second_republish_time),
            "Republish time was mutated by a past republish request"
        );
    }
}
