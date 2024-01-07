use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Formatter, write};

use crate::domain::dht::expiring::Expiring;
use crate::domain::dht::hash_table::LocalHashTable;

use crate::domain::dht::strategies::insert_strategy::InsertionStrategy;
use crate::domain::dht::strategies::fetch_strategy::FetchStrategy;
use crate::domain::dht::strategies::timeout_strategy::TimeoutStrategy;

#[derive(Clone)]
pub struct ExpiringHashTable<H, D, IS, FS, TS>
{
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

impl<H, I, O, D, IS, FS, TS, RS, FE> LocalHashTable<H, I, O> for ExpiringHashTable<H, D, IS, FS, TS>
    where IS: InsertionStrategy<Handle=H, Composite=HashMap<H, D>, InputData=I, Status=RS>,
          FS: FetchStrategy<Handle=H, Composite=HashMap<H, D>, OutputData=O, Error=FE>
{
    type StoreRes = RS;
    type FetchErr = FE;

    fn store(&mut self, handle: H, data: I) -> Self::StoreRes {
        self.insertion_strategy.insert(handle, data, &mut self.map)
    }
    fn fetch(&mut self, handle: &H) -> Result<O, Self::FetchErr> {
        self.fetch_strategy.fetch(handle, &mut self.map)
    }
}

// todo maybe we can even remove the concrete HashSet
// todo can we put this into a separat strategy we can test?
impl<H, D, IS, FS, TS> Expiring for ExpiringHashTable<H, HashSet<D>, IS, FS, TS> where
    TS: TimeoutStrategy<Context=H, Expirable=D>
{
    type Context = ();
    type Result = ();

    fn expire(&mut self, context: &Self::Context) -> Self::Result {
        for (handle, set) in self.map.iter_mut() {
            set.retain(|data| !self.timeout_strategy.has_timed_out(handle, data));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use crate::domain::dht::expiring::Expiring;
    use crate::domain::dht::hash_table::expiring_hash_table::ExpiringHashTable;
    use crate::domain::dht::strategies::timeout_strategy::{TaggedTimeoutStrategy, TaggedValue};

    #[test]
    fn expire_value_if_timed_out() {
        let mut expiring = ExpiringHashTable::new((), (), TaggedTimeoutStrategy::default());
        expiring.map.insert("test", HashSet::from([TaggedValue {
            value: "test", tagged: true
        }]));

        Expiring::expire(&mut expiring, &());

        todo!("Specify behaviour: Do we want to leave empty Sets behind?");
        assert!(expiring.map.is_empty(), "Hash table is not empty: {:?}", expiring.map);
    }

    #[test]
    fn dont_expire_fresh_values() {
        let mut expiring = ExpiringHashTable::new((), (), TaggedTimeoutStrategy::default());
        let data = HashSet::from([TaggedValue {
            value: "test", tagged: false
        }]);
        expiring.map.insert("test", data.clone());

        Expiring::expire(&mut expiring, &());

        let expired_data = expiring.map.get("test");
        assert!(expired_data.is_some(), "Hash table doesn't contain data for 'test': {:?}", expiring.map);

        let expired_data = expired_data.unwrap();
        assert_eq!(expired_data, &data, "Some data was expired: {:?}", expired_data);
    }
}

