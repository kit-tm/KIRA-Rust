pub use permissionless_insert_strategy::PermissionlessInsertStrategy;

mod permissionless_insert_strategy;

/// This trait defines methods to insert data with a given handle into composite object.
pub trait InsertionStrategy {
    /// The handle used to insert data into the [Self::Composite]
    type Handle;

    /// The target into which to insert
    type Composite;

    /// Data which shall be inserted
    type InputData;

    /// Status if the insertion succeeded
    type Status;

    /// Inserts the provided data into the composite object.
    ///
    /// # Parameters
    ///
    /// - `handle`: A handle to identify the data being inserted.
    /// - `data`: The input data to be inserted.
    /// - `into`: A mutable reference to the composite object where the data is inserted.
    ///
    /// # Returns
    ///
    /// Returns the status of the insertion operation.
    fn insert(
        &self,
        handle: Self::Handle,
        data: Self::InputData,
        into: &mut Self::Composite,
    ) -> Self::Status;
}
