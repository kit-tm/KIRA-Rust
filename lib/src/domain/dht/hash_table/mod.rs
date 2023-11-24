pub mod expiring_hash_table;

pub trait LocalHashTable<H, I, O>
{
    type StoreRes;
    type FetchErr;

    fn store(&mut self, handle: H, data: I) -> Self::StoreRes;
    fn fetch(&mut self, handle: &H) -> Result<O, Self::FetchErr>;
}