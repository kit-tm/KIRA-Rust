use std::collections::HashMap;
use std::hash::Hash;


use crate::domain::dht::Expiring;
use crate::domain::dht::{TimeoutStrategy, ContextExpiring};
use crate::domain::dht::hash_table::{FetchStrategy, HashTable, InsertStrategy};
use crate::messaging::dht::{StoreOK, StoreErr, FetchErr};

pub struct ExpiringHashTable<H, D> {
    map: HashMap<H, D>,
}

impl<H, D> Expiring<H> for ExpiringHashTable<H, D>
    where
        H: Eq + Hash,
        D: ContextExpiring<H>
{
    fn collect(&mut self, strategy: &impl TimeoutStrategy<H>) -> bool {
        let mut is_empty = true;
        for (handle, data) in self.map.iter_mut() {
            if data.collect_with_context(handle, strategy) {
                self.map.remove(handle)
            } else {
                is_empty = false;
            }
        }

        is_empty
    }
}

impl<H, I, O, D> HashTable<H, I, O> for ExpiringHashTable<H, D>
    where H: Eq + Hash,
          D: Eq + Hash,
          I: InsertStrategy<D, InsertOk=Self::StoreOK, InsertErr=Self::StoreErr> + Into<D>,
          O: FetchStrategy<D, FetchErr=Self::FetchErr>
{
    type StoreErr = StoreErr;
    type StoreOK = StoreOK;
    type FetchErr = FetchErr;

    fn store(&mut self, handle: H, data: I) -> Result<Self::StoreOK, Self::StoreErr> {
        match self.map.get_mut(&handle) {
            None => {
                self.map.insert(handle, data.into());
                Ok(StoreOK::Created)
            }
            Some(existing_data) => {
                data.insert(existing_data)
            }
        }
    }
    fn fetch(&self, handle: &H) -> Result<O, Self::FetchErr> {
        match self.map.get(handle) {
            None => Err(FetchErr::NotFoundErr),
            Some(intern_data) => O::from(intern_data)
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