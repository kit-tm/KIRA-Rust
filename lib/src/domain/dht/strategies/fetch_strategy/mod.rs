pub use permissionless_fetch_strategy::PermissionlessFetchStrategy;
mod permissionless_fetch_strategy;

pub trait FetchStrategy {
    type Handle;
    type Composite;

    type OutputData;
    type Error;

    fn fetch(&self, handle: &Self::Handle, from: &mut Self::Composite) -> Result<Self::OutputData, Self::Error>;
    
    fn fetch_all(&self, from: &Self::Composite) -> Result<Vec<(Self::Handle, Self::OutputData)>, Self::Error>;
}