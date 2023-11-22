pub trait InsertStrategy<D> {
    type InsertOk;
    type InsertErr;

    fn insert(self, into: &mut D) -> Result<Self::InsertOk, Self::InsertErr>;
}

pub trait FetchStrategy<D> {
    type FetchErr;

    fn fetch(from: &D) -> Result<&Self, Self::FetchErr>;
}

pub trait HashTable<H, I, O>
{
    type StoreErr;
    type StoreOK;
    type FetchErr;

    fn store(&mut self, handle: H, data: I) -> Result<Self::StoreOK, Self::StoreErr>;
    fn fetch(&self, handle: &H) -> Result<&O, Self::FetchErr>;
}