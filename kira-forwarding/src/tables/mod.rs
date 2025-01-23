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
pub trait NodeIdTable {
    #[allow(missing_docs)]
    type Error: std::fmt::Debug;

    /// Creates the given [NodeIdEntry].
    ///
    /// Emits an error if an entry with the entries `destination` already exists.
    fn create(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error>;
    /// Updates an existing [NodeIdEntry].
    ///
    /// Emits an error if the entries destination [NodeId] doesn't yet exist.
    fn update(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error>;
    /// Creates the given [NodeIdEntry] if it doesn't exist yet, otherwise updates it.
    fn create_or_update(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error>;
    /// Removes a [NodeIdEntry].
    ///
    /// Doesn't emit an error if the entries destination [NodeId] doesn't exist.
    ///
    /// Returns the removed [NodeIdEntry].
    fn remove(&mut self, node_id: &NodeIdSubnet) -> Result<Option<NodeIdEntry>, Self::Error>;
}

/// CRUD access interface to the forwarding table based on [PathId]s.
pub trait PathIdTable {
    #[allow(missing_docs)]
    type Error: std::fmt::Debug;

    /// Creates the given [PathIdEntry].
    ///
    /// Emits an error if an entry with the entries `in_path_id` already exists.
    fn create(&mut self, entry: PathIdEntry) -> Result<(), Self::Error>;
    /// Updates an existing [PathIdEntry].
    ///
    /// Emits an error if the entries `in_path_id` doesn't yet exist.
    fn update(&mut self, entry: PathIdEntry) -> Result<(), Self::Error>;
    /// Creates the given [PathIdEntry] if it doesn't exist yet, otherwise updates it.
    fn create_or_update(&mut self, entry: PathIdEntry) -> Result<(), Self::Error>;
    /// Removes a [PathIdEntry].
    ///
    /// **Doesn't** emit an error if the entries `in_path_id` doesn't exist.
    ///
    /// Returns the removed [PathIdEntry].
    fn remove(&mut self, path_id: &PathId) -> Result<Option<PathIdEntry>, Self::Error>;
}

/// CRUD access interface to the whole forwarding layer and its forwarding tables.
pub trait ForwardingTables: NodeIdTable + PathIdTable {}

/// Handle an [ForwardingTablesUpdate](r2kad::ForwardingTablesUpdate) by
/// delegating it to the `table`.
///
/// # Note
///
/// The result of [NodeIdTable::remove] and [PathIdTable::remove] are ignored.
pub fn handle_r2kad_request<FT, E>(
    tables: &mut FT,
    req: r2kad::ForwardingTablesUpdate,
) -> Result<(), E>
where
    // ForwardingTables with common error type
    FT: ForwardingTables,
    FT: NodeIdTable<Error = E>,
    FT: PathIdTable<Error = E>,
{
    match req {
        NodeIdTableUpdate(update) => {
            use r2kad::NodeIdTableUpdate::*;

            match update {
                Create(entry) => NodeIdTable::create(tables, entry),
                Update(entry) => NodeIdTable::update(tables, entry),
                CreateOrUpdate(entry) => NodeIdTable::create_or_update(tables, entry),
                Remove(subnet) => {
                    let _ = NodeIdTable::remove(tables, &subnet)?;
                    Ok(())
                }
            }
        }
        PathIdTableUpdate(update) => {
            use r2kad::PathIdTableUpdate::*;

            match update {
                Create(entry) => PathIdTable::create(tables, entry),
                Update(entry) => PathIdTable::update(tables, entry),
                CreateOrUpdate(entry) => PathIdTable::create_or_update(tables, entry),
                Remove(path_id) => {
                    let _ = PathIdTable::remove(tables, &path_id)?;
                    Ok(())
                }
            }
        }
    }
}
