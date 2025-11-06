use std::collections::hash_map::Keys;
use std::collections::{HashMap, HashSet};

use crate::domain::dht::expiring::Expiring;
use crate::domain::dht::hash_table::LocalHashTable;

use crate::domain::dht::strategies::fetch_strategy::FetchStrategy;
use crate::domain::dht::strategies::insert_strategy::InsertionStrategy;
use crate::domain::dht::strategies::timeout_strategy::TimeoutStrategy;

#[derive(Debug, Clone)]
pub struct ExpiringHashTable<H, D, IS, FS, TS> {
    pub map: HashMap<H, D>,
    pub insertion_strategy: IS,
    pub fetch_strategy: FS,
    pub timeout_strategy: TS,
}

impl<H, D, IS, FS, TS> ExpiringHashTable<H, D, IS, FS, TS> {
    pub fn new(insertion_strategy: IS, fetch_strategy: FS, timeout_strategy: TS) -> Self {
        Self {
            map: HashMap::default(),
            insertion_strategy,
            fetch_strategy,
            timeout_strategy,
        }
    }
}

impl<H, D, IS: Default, FS: Default, TS: Default> Default for ExpiringHashTable<H, D, IS, FS, TS> {
    fn default() -> Self {
        Self::new(Default::default(), Default::default(), Default::default())
    }
}

impl<H, I, O, D, IS, FS, TS, RS, FE> LocalHashTable<H, I, O> for ExpiringHashTable<H, D, IS, FS, TS>
where
    IS: InsertionStrategy<Handle = H, Composite = HashMap<H, D>, InputData = I, Status = RS>,
    FS: FetchStrategy<Handle = H, Composite = HashMap<H, D>, OutputData = O, Error = FE>,
{
    type StoreRes = RS;
    type FetchErr = FE;
    type InternalData = D;

    fn store(&mut self, handle: H, data: I) -> Self::StoreRes {
        self.insertion_strategy.insert(handle, data, &mut self.map)
    }

    fn peek(&self, handle: &H) -> Result<O, Self::FetchErr> {
        self.fetch_strategy.peek(handle, &self.map)
    }

    fn fetch(&mut self, handle: &H) -> Result<O, Self::FetchErr> {
        self.fetch_strategy.fetch(handle, &mut self.map)
    }

    fn fetch_all(&self) -> Result<Vec<(H, O)>, Self::FetchErr> {
        self.fetch_strategy.fetch_all(&self.map)
    }

    fn handles(&self) -> Keys<'_, H, Self::InternalData> {
        self.map.keys()
    }
}

// TODO: maybe we can even remove the concrete HashSet
// TODO: can we put this into a separate strategy we can test?
impl<H, D, IS, FS, TS> Expiring for ExpiringHashTable<H, HashSet<D>, IS, FS, TS>
where
    TS: TimeoutStrategy<Context = H, Expirable = D>,
{
    type Context = ();
    type Result = ();

    fn expire(&mut self, _context: &Self::Context) -> Self::Result {
        for (handle, set) in self.map.iter_mut() {
            set.retain(|data| !self.timeout_strategy.has_timed_out(handle, data));
        }

        // delete empty sets left behind
        self.map.retain(|_, set| !set.is_empty());
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::dht::expiring::Expiring;
    use crate::domain::dht::hash_table::expiring_hash_table::ExpiringHashTable;
    use crate::domain::dht::strategies::timeout_strategy::{TaggedTimeoutStrategy, TaggedValue};
    use std::collections::HashSet;

    #[test]
    fn expire_whole_data_set_if_timed_out() {
        let mut expiring = ExpiringHashTable::new((), (), TaggedTimeoutStrategy::default());
        let tagged_value = TaggedValue::new("test").tag();
        expiring.map.insert("test", HashSet::from([tagged_value]));

        Expiring::expire(&mut expiring, &());

        assert!(
            expiring.map.is_empty(),
            "Hash table is not empty: {:?}",
            expiring.map
        );
    }

    #[test]
    fn dont_expire_fresh_values() {
        let mut expiring = ExpiringHashTable::new((), (), TaggedTimeoutStrategy::default());
        let fresh_value = TaggedValue::new("test");
        let data = HashSet::from([fresh_value]);
        expiring.map.insert("test", data.clone());

        Expiring::expire(&mut expiring, &());

        let expired_data = expiring.map.get("test");
        assert!(
            expired_data.is_some(),
            "Hash table doesn't contain data for 'test': {:?}",
            expiring.map
        );

        let expired_data = expired_data.unwrap();
        assert_eq!(
            expired_data, &data,
            "Some data was expired: {expired_data:?}"
        );
    }

    #[test]
    fn expire_only_expired_values() {
        let mut expiring = ExpiringHashTable::new((), (), TaggedTimeoutStrategy::default());
        let key = "test";
        let fresh_value = TaggedValue::new("test");
        let tagged_value = TaggedValue::new("tested").tag();
        expiring
            .map
            .insert(key, HashSet::from([fresh_value, tagged_value]));

        Expiring::expire(&mut expiring, &());

        assert!(
            !expiring.map.is_empty(),
            "Hash table is empty: {:?}",
            expiring.map
        );
        assert!(
            expiring.map.contains_key("test"),
            "Hash table does not contain fresh value: {:?}",
            expiring.map
        );
        assert!(
            !expiring.map.contains_key("tested"),
            "Hash table does contain expired value: {:?}",
            expiring.map
        );
    }
}
