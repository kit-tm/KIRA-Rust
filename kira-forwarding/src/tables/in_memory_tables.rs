//! Dummy [AsyncForwardingTables] implementation just storing the forwarding entries.
//!
//! The main struct is the [InMemoryFwdTables].

use std::collections::HashMap;

use crate::{
    domain::{
        NodeId,
        NodeIdSubnet,
        PathId,
    },
    tables::{
        AsyncForwardingTables,
        AsyncNodeIdTable,
        AsyncPathIdTable,
        NodeIdEntry,
        PathIdEntry,
    },
};

/// In-Memory [AsyncForwardingTables] implementation backed by [HashMap]s.
///
/// The implementation *does not* provide any fast forwarding since it just stores all entries.
/// Also logs every change to the forwarding tables with log target `in_memory_fwd_table`.
#[derive(Debug, Default)]
pub struct InMemoryFwdTables {
    node_id_table: HashMap<NodeIdSubnet, NodeIdEntry>,
    path_id_table: HashMap<PathId, PathIdEntry>,
}

impl InMemoryFwdTables {
    /// Returns the entry by [NodeId].
    pub fn node_id_entry(&self, node_id: &NodeId) -> Option<&NodeIdEntry> {
        self.node_id_table.get(&NodeIdSubnet::new(*node_id))
    }

    /// Returns the entry by incoming [PathId].
    pub fn path_id_entry(&self, path_id: &PathId) -> Option<&PathIdEntry> {
        self.path_id_table.get(path_id)
    }
}

impl AsyncNodeIdTable for InMemoryFwdTables {
    type Error = error::FwdTableError;

    async fn create(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error> {
        let destination = match entry {
            NodeIdEntry::Forward(ref entry) => entry.destination.clone(),
            NodeIdEntry::Encapsulate(ref entry) => entry.destination.clone(),
        };

        if self.node_id_table.contains_key(&destination) {
            return Err(error::FwdTableError::EntryAlreadyExists);
        }

        log::trace!(target: "in_memory_fwd_table", "Created {entry:?}");

        self.node_id_table.insert(destination, entry);

        Ok(())
    }

    async fn update(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error> {
        let destination = match entry {
            NodeIdEntry::Forward(ref entry) => entry.destination.clone(),
            NodeIdEntry::Encapsulate(ref entry) => entry.destination.clone(),
        };
        if let Some(old_entry) = self.node_id_table.get_mut(&destination) {
            log::trace!(target: "in_memory_fwd_table", "Updated old: {old_entry:?}, new: {entry:?}");
            *old_entry = entry;
            Ok(())
        } else {
            Err(error::FwdTableError::EntryMissing)
        }
    }

    async fn remove(&mut self, node_id: &NodeIdSubnet) -> Result<Option<NodeIdEntry>, Self::Error> {
        if let Some(removed) = self.node_id_table.remove(node_id) {
            log::trace!(target: "in_memory_fwd_table", "Removed {removed:?}");
            Ok(Some(removed))
        } else {
            Ok(None)
        }
    }

    async fn create_or_update(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error> {
        let destination = match entry {
            NodeIdEntry::Forward(ref entry) => entry.destination.clone(),
            NodeIdEntry::Encapsulate(ref entry) => entry.destination.clone(),
        };
        if self.node_id_table.contains_key(&destination) {
            AsyncNodeIdTable::update(self, entry).await?;
        } else {
            AsyncNodeIdTable::create(self, entry).await?;
        }
        Ok(())
    }
}

impl AsyncPathIdTable for InMemoryFwdTables {
    type Error = error::FwdTableError;

    async fn create(&mut self, entry: PathIdEntry) -> Result<(), Self::Error> {
        let in_path_id = match entry {
            PathIdEntry::Decapsulate(ref entry) => entry.in_path_id.clone(),
            PathIdEntry::Forward(ref entry) => entry.in_path_id.clone(),
        };

        if self.path_id_table.contains_key(&in_path_id) {
            return Err(error::FwdTableError::EntryAlreadyExists);
        }

        log::trace!(target: "in_memory_fwd_table", "Created {entry:?}");

        self.path_id_table.insert(in_path_id, entry);

        Ok(())
    }

    async fn update(&mut self, entry: PathIdEntry) -> Result<(), Self::Error> {
        let in_path_id = match entry {
            PathIdEntry::Decapsulate(ref entry) => entry.in_path_id.clone(),
            PathIdEntry::Forward(ref entry) => entry.in_path_id.clone(),
        };

        if let Some(old_entry) = self.path_id_table.get_mut(&in_path_id) {
            log::trace!(target: "in_memory_fwd_table", "Updated old: {old_entry:?}, new: {entry:?}");
            *old_entry = entry;
            Ok(())
        } else {
            Err(error::FwdTableError::EntryMissing)
        }
    }

    async fn remove(&mut self, path_id: &PathId) -> Result<Option<PathIdEntry>, Self::Error> {
        if let Some(removed) = self.path_id_table.remove(path_id) {
            log::trace!(target: "in_memory_fwd_table", "Removed {removed:?}");
            Ok(Some(removed))
        } else {
            Ok(None)
        }
    }

    async fn create_or_update(&mut self, entry: PathIdEntry) -> Result<(), Self::Error> {
        let in_path_id = match entry {
            PathIdEntry::Decapsulate(ref entry) => entry.in_path_id.clone(),
            PathIdEntry::Forward(ref entry) => entry.in_path_id.clone(),
        };

        if self.path_id_table.contains_key(&in_path_id) {
            AsyncPathIdTable::update(self, entry).await
        } else {
            AsyncPathIdTable::create(self, entry).await
        }
    }
}

impl AsyncForwardingTables for InMemoryFwdTables {}

impl Extend<NodeIdEntry> for InMemoryFwdTables {
    fn extend<T: IntoIterator<Item = NodeIdEntry>>(&mut self, iter: T) {
        let entries = iter.into_iter().map(|entry| {
            let destination = match entry {
                NodeIdEntry::Forward(ref entry) => entry.destination.clone(),
                NodeIdEntry::Encapsulate(ref entry) => entry.destination.clone(),
            };
            (destination, entry)
        });
        self.node_id_table.extend(entries);
    }
}

impl Extend<PathIdEntry> for InMemoryFwdTables {
    fn extend<T: IntoIterator<Item = PathIdEntry>>(&mut self, iter: T) {
        let entries = iter.into_iter().map(|entry| {
            let in_path_id = match entry {
                PathIdEntry::Decapsulate(ref entry) => entry.in_path_id.clone(),
                PathIdEntry::Forward(ref entry) => entry.in_path_id.clone(),
            };
            (in_path_id, entry)
        });
        self.path_id_table.extend(entries);
    }
}

pub mod error {
    //! Errors for [InMemoryFwdTables](super::InMemoryFwdTables).

    use derive_more::derive::{
        Display,
        Error,
    };

    #[derive(Debug, Display, Error)]
    /// Error for [InMemoryFwdTables] used collectively
    /// in the [AyncNodeIdTable] and the [AsyncPathIdTable] trait implementation.
    ///
    /// [InMemoryFwdTables]: super::InMemoryFwdTables
    /// [AyncNodeIdTable]: super::super::AsyncNodeIdTable
    /// [AsyncPathIdTable]: super::super::AsyncPathIdTable
    pub enum FwdTableError {
        #[display("Entry  already exists")]
        /// If the exact entry is already contained in the [InMemoryFwdTables]
        /// and tried to added using the `create` methods this error is returned.
        ///
        /// [InMemoryFwdTables]: super::InMemoryFwdTables
        EntryAlreadyExists,
        #[display("Entry with id  doesn't exist")]
        /// Retrieving an entry with the given id was unsuccessful because
        /// the [InMemoryFwdTables] does not has any entry stored under the given id.
        ///
        /// [InMemoryFwdTables]: super::InMemoryFwdTables
        EntryMissing,
    }
}
