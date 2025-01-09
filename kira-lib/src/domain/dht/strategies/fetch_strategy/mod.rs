pub use permissionless_fetch_strategy::PermissionlessFetchStrategy;
mod permissionless_fetch_strategy;

/// This trait defines methods to fetch data based on a given handle from a composite object.
/// It also provides methods to retrieve all data from a composite object.
pub trait FetchStrategy {
    /// The handle used to retrieve data from the [Self::Composite]
    type Handle;

    /// The target of the fetch request.
    ///
    /// From here the strategy tries to extract the requested value
    /// via the provided [Self::Handle]
    type Composite;

    /// The data sent on a successful fetch
    ///
    /// This is usually the type stored in the [Self::Composite]
    /// but the strategy can also perform a transformation
    /// to obfuscate the real type being used.
    type OutputData;

    /// The error returned if the data hasn't been found
    type Error;

    /// Extracts data from the [Self::Composite].
    ///
    /// # Arguments
    ///
    /// * `handle` - A reference to the handle used to find the data in the composite.
    /// * `from` - A mutable reference to the composite where the data should be fetched from.
    ///
    /// # Returns
    ///
    /// A `Result` that contains the fetched data if successful, or an error if the data couldn't be found in the [Self:Composite].
    fn fetch(
        &self,
        handle: &Self::Handle,
        from: &mut Self::Composite,
    ) -> Result<Self::OutputData, Self::Error>;

    /// Extracts data from the [Self::Composite].
    ///
    /// In contrast to [fetch](Self::fetch) this method **does not alter** the composite data.
    ///
    /// # Arguments
    ///
    /// * `handle` - A reference to the handle used to find the data in the composite.
    /// * `from` - A mutable reference to the composite where the data should be fetched from.
    ///
    /// # Returns
    ///
    /// A `Result` that contains the fetched data if successful, or an error if the data couldn't be found in the [Self:Composite].
    fn peek(
        &self,
        handle: &Self::Handle,
        from: &Self::Composite,
    ) -> Result<Self::OutputData, Self::Error>;

    /// Extracts **all** data from the composite object.
    ///
    /// This method should perform the same conversion to the [Self::OutputData] as [Self::fetch] if necessary.
    /// The output data has to be tagged with the [Self::Handle] by wrapping it in a tuple.
    ///
    /// # Parameters
    ///
    /// - `from` : A reference to the composite object from which to extrat the data.
    ///
    /// # Returns
    ///
    /// Returns a `Result` object containing either a vector of [Self::Handle] tagged [Self::OutputData] or an [Self::Error]
    /// if something failed on retrieving all data.
    #[allow(clippy::type_complexity)]
    fn fetch_all(
        &self,
        from: &Self::Composite,
    ) -> Result<Vec<(Self::Handle, Self::OutputData)>, Self::Error>;
}
