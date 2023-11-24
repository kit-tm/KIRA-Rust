pub use permissionless_insert_strategy::PermissionlessInsertStrategy;

mod permissionless_insert_strategy;

pub trait InsertionStrategy {
    type Handle;
    type Composite;

    type InputData;
    type Status;

    fn insert(&self, handle: Self::Handle, data: Self::InputData, into: &mut Self::Composite) -> Self::Status;
}