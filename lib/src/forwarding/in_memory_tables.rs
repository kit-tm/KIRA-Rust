use std::collections::HashMap;

use crate::domain::path_id::PathId;
use crate::domain::NodeId;
use crate::forwarding::{ForwardingTables, NodeIdEntry, NodeIdTable, PathIdEntry, PathIdTable};

/// In-Memory [ForwardingTables] implementation backed by [HashMap]s.
///
/// Also logs every change to the forwarding tables with log target `in_memory_fwd_table`.
#[derive(Debug, Default)]
pub struct InMemoryFwdTables {
    node_id_table: HashMap<NodeId, NodeIdEntry>,
    path_id_table: HashMap<PathId, PathIdEntry>,
}

impl InMemoryFwdTables {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn node_id_entry(&self, node_id: &NodeId) -> Option<&NodeIdEntry> {
        self.node_id_table.get(node_id)
    }

    pub fn path_id_entry(&self, path_id: &PathId) -> Option<&PathIdEntry> {
        self.path_id_table.get(path_id)
    }
}

impl NodeIdTable for InMemoryFwdTables {
    type Error = error::FwdTableError;

    fn create(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error> {
        if self.node_id_table.contains_key(&entry.destination) {
            return Err(error::FwdTableError::EntryAlreadyExists);
        }

        log::trace!(target: "in_memory_fwd_table", "Created {:?}", entry);

        self.node_id_table.insert(entry.destination.clone(), entry);

        Ok(())
    }

    fn update(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error> {
        if let Some(old_entry) = self.node_id_table.get_mut(&entry.destination) {
            log::trace!(target: "in_memory_fwd_table", "Updated old: {:?}, new: {:?}", old_entry, entry);
            *old_entry = entry;
            Ok(())
        } else {
            Err(error::FwdTableError::EntryMissing)
        }
    }

    fn remove(&mut self, node_id: &NodeId) -> Result<Option<NodeIdEntry>, Self::Error> {
        if let Some(removed) = self.node_id_table.remove(node_id) {
            log::trace!(target: "in_memory_fwd_table", "Removed {:?}", removed);
            Ok(Some(removed))
        } else {
            Ok(None)
        }
    }
}

impl PathIdTable for InMemoryFwdTables {
    type Error = error::FwdTableError;

    fn create(&mut self, entry: PathIdEntry) -> Result<(), Self::Error> {
        if self.path_id_table.contains_key(&entry.in_path_id) {
            return Err(error::FwdTableError::EntryAlreadyExists);
        }

        log::trace!(target: "in_memory_fwd_table", "Created {:?}", entry);

        self.path_id_table.insert(entry.in_path_id.clone(), entry);

        Ok(())
    }

    fn update(&mut self, entry: PathIdEntry) -> Result<(), Self::Error> {
        if let Some(old_entry) = self.path_id_table.get_mut(&entry.in_path_id) {
            log::trace!(target: "in_memory_fwd_table", "Updated old: {:?}, new: {:?}", old_entry, entry);
            *old_entry = entry;
            Ok(())
        } else {
            Err(error::FwdTableError::EntryMissing)
        }
    }

    fn remove(&mut self, path_id: &PathId) -> Result<Option<PathIdEntry>, Self::Error> {
        if let Some(removed) = self.path_id_table.remove(path_id) {
            log::trace!(target: "in_memory_fwd_table", "Removed {:?}", removed);
            Ok(Some(removed))
        } else {
            Ok(None)
        }
    }
}

impl ForwardingTables for InMemoryFwdTables {}

impl Extend<NodeIdEntry> for InMemoryFwdTables {
    fn extend<T: IntoIterator<Item = NodeIdEntry>>(&mut self, iter: T) {
        let entries = iter
            .into_iter()
            .map(|entry| (entry.destination.clone(), entry));
        self.node_id_table.extend(entries);
    }
}

impl Extend<PathIdEntry> for InMemoryFwdTables {
    fn extend<T: IntoIterator<Item = PathIdEntry>>(&mut self, iter: T) {
        let entries = iter
            .into_iter()
            .map(|entry| (entry.in_path_id.clone(), entry));
        self.path_id_table.extend(entries);
    }
}

pub mod error {
    use std::error::Error;
    use std::fmt::{Display, Formatter};

    #[derive(Debug)]
    pub enum FwdTableError {
        EntryAlreadyExists,
        EntryMissing,
    }

    impl Display for FwdTableError {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::EntryAlreadyExists => write!(f, "Entry already exists"),
                Self::EntryMissing => write!(f, "Entry with id doesn't exist"),
            }
        }
    }

    impl Error for FwdTableError {}
}
