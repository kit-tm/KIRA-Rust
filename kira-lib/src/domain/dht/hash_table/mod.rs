use std::collections::hash_map::Keys;

pub mod expiring_hash_table;

pub trait LocalHashTable<H, I, O> {
    type StoreRes;
    type FetchErr;
    type InternalData;

    // TODO: document this
    fn store(&mut self, handle: H, data: I) -> Self::StoreRes;
    fn peek(&self, handle: &H) -> Result<O, Self::FetchErr>;
    fn fetch(&mut self, handle: &H) -> Result<O, Self::FetchErr>;
    fn fetch_all(&self) -> Result<Vec<(H, O)>, Self::FetchErr>;
    fn handles(&self) -> Keys<'_, H, Self::InternalData>;
}
