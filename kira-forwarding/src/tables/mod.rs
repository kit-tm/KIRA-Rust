//! Type definitions for the forwarding layer interface.

pub mod in_memory_tables;
#[cfg(feature = "nft")]
pub mod native_tables;

use crate::domain::r2kad::{
    self,
    ForwardingTablesUpdate::{NodeIdTableUpdate, PathIdTableUpdate},
    NodeIdEntry, PathIdEntry,
};
use crate::domain::{NodeIdSubnet, PathId};

/// CRUD access interface to the forwarding table based on [NodeIds](crate::domain::NodeId).
#[trait_variant::make(AsyncNodeIdTable: Send)]
pub trait LocalAsyncNodeIdTable {
    #[allow(missing_docs)]
    type Error: std::fmt::Debug;

    /// Creates the given [NodeIdEntry].
    ///
    /// Emits an error if an entry with the entries `destination` already exists.
    async fn create(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error>;
    /// Updates an existing [NodeIdEntry].
    ///
    /// Emits an error if the entries destination [NodeId](crate::domain::NodeId) doesn't yet exist.
    async fn update(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error>;
    /// Creates the given [NodeIdEntry] if it doesn't exist yet, otherwise updates it.
    async fn create_or_update(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error>;
    /// Removes a [NodeIdEntry].
    ///
    /// Doesn't emit an error if the entries destination [NodeId](crate::domain::NodeId) doesn't exist.
    ///
    /// Returns the removed [NodeIdEntry].
    async fn remove(&mut self, node_id: &NodeIdSubnet) -> Result<Option<NodeIdEntry>, Self::Error>;
}

/// CRUD access interface to the forwarding table based on [PathId]s.
#[trait_variant::make(AsyncPathIdTable: Send)]
pub trait LocalAsyncPathIdTable {
    #[allow(missing_docs)]
    type Error: std::fmt::Debug;

    /// Creates the given [PathIdEntry].
    ///
    /// Emits an error if an entry with the entries `in_path_id` already exists.
    async fn create(&mut self, entry: PathIdEntry) -> Result<(), Self::Error>;
    /// Updates an existing [PathIdEntry].
    ///
    /// Emits an error if the entries `in_path_id` doesn't yet exist.
    async fn update(&mut self, entry: PathIdEntry) -> Result<(), Self::Error>;
    /// Creates the given [PathIdEntry] if it doesn't exist yet, otherwise updates it.
    async fn create_or_update(&mut self, entry: PathIdEntry) -> Result<(), Self::Error>;
    /// Removes a [PathIdEntry].
    ///
    /// **Doesn't** emit an error if the entries `in_path_id` doesn't exist.
    ///
    /// Returns the removed [PathIdEntry].
    async fn remove(&mut self, path_id: &PathId) -> Result<Option<PathIdEntry>, Self::Error>;
}

/// CRUD access interface to the whole forwarding layer and its forwarding tables.
pub trait LocalAsyncForwardingTables: LocalAsyncNodeIdTable + LocalAsyncPathIdTable {}
/// CRUD access interface to the whole forwarding layer and its forwarding tables.
pub trait AsyncForwardingTables: AsyncNodeIdTable + AsyncPathIdTable {}

/// Handle an [ForwardingTablesUpdate](r2kad::ForwardingTablesUpdate) by
/// delegating it to the `table`.
///
/// # Note
///
/// The result of [AsyncNodeIdTable::remove] and [AsyncPathIdTable::remove] are ignored.
pub async fn handle_r2kad_request<FT, E>(
    tables: &mut FT,
    req: r2kad::ForwardingTablesUpdate,
) -> Result<(), E>
where
    // ForwardingTables with common error type
    FT: AsyncForwardingTables,
    FT: AsyncNodeIdTable<Error = E>,
    FT: AsyncPathIdTable<Error = E>,
{
    match req {
        NodeIdTableUpdate(update) => {
            use r2kad::NodeIdTableUpdate::*;

            match update {
                Create(entry) => AsyncNodeIdTable::create(tables, entry).await,
                Update(entry) => AsyncNodeIdTable::update(tables, entry).await,
                CreateOrUpdate(entry) => AsyncNodeIdTable::create_or_update(tables, entry).await,
                Remove(subnet) => AsyncNodeIdTable::remove(tables, &subnet).await.map(|_| ()),
            }
        }
        PathIdTableUpdate(update) => {
            use r2kad::PathIdTableUpdate::*;

            match update {
                Create(entry) => AsyncPathIdTable::create(tables, entry).await,
                Update(entry) => AsyncPathIdTable::update(tables, entry).await,
                CreateOrUpdate(entry) => AsyncPathIdTable::create_or_update(tables, entry).await,
                Remove(path_id) => AsyncPathIdTable::remove(tables, &path_id).await.map(|_| ()),
            }
        }
    }
}
