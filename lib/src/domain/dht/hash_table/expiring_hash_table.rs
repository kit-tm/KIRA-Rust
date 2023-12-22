use std::collections::{HashMap, HashSet};

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
