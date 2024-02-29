use std::collections::HashMap;

use crate::domain::path_id::PathId;
use crate::domain::{NodeId, NodeIdSubnet};
use crate::forwarding::{ForwardingTables, NodeIdEntry, NodeIdTable, PathIdEntry, PathIdTable};

/// In-Memory [ForwardingTables] implementation backed by [HashMap]s.
///
/// Also logs every change to the forwarding tables with log target `in_memory_fwd_table`.
#[derive(Debug, Default)]
pub struct InMemoryFwdTables {
    node_id_table: HashMap<NodeIdSubnet, NodeIdEntry>,
    path_id_table: HashMap<PathId, PathIdEntry>,
}

impl InMemoryFwdTables {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the entry by [NodeId].
    pub fn node_id_entry(&self, node_id: &NodeId) -> Option<&NodeIdEntry> {
        self.node_id_table.get(&NodeIdSubnet { node_id: node_id.clone(), prefix_length: 0 })
    }

    /// Returns the entry by incoming [PathId].
    pub fn path_id_entry(&self, path_id: &PathId) -> Option<&PathIdEntry> {
        self.path_id_table.get(path_id)
    }
}

impl NodeIdTable for InMemoryFwdTables {
    type Error = error::FwdTableError;

    fn create(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error> {
        let destination = match entry {
            NodeIdEntry::Forward(ref entry) => entry.destination.clone(),
            NodeIdEntry::Encapsulate(ref entry) => entry.destination.clone(),
        };

        if self.node_id_table.contains_key(&destination) {
            return Err(error::FwdTableError::EntryAlreadyExists);
        }

        log::trace!(target: "in_memory_fwd_table", "Created {:?}", entry);

        self.node_id_table.insert(destination, entry);

        Ok(())
    }

    fn update(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error> {
        let destination = match entry {
            NodeIdEntry::Forward(ref entry) => entry.destination.clone(),
            NodeIdEntry::Encapsulate(ref entry) => entry.destination.clone(),
        };
        if let Some(old_entry) = self.node_id_table.get_mut(&destination) {
            log::trace!(target: "in_memory_fwd_table", "Updated old: {:?}, new: {:?}", old_entry, entry);
            *old_entry = entry;
            Ok(())
        } else {
            Err(error::FwdTableError::EntryMissing)
        }
    }

    fn remove(&mut self, node_id: &NodeIdSubnet) -> Result<Option<NodeIdEntry>, Self::Error> {
        if let Some(removed) = self.node_id_table.remove(node_id) {
            log::trace!(target: "in_memory_fwd_table", "Removed {:?}", removed);
            Ok(Some(removed))
        } else {
            Ok(None)
        }
    }

    fn create_or_update(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error> {
        let destination = match entry {
            NodeIdEntry::Forward(ref entry) => entry.destination.clone(),
            NodeIdEntry::Encapsulate(ref entry) => entry.destination.clone(),
        };
        if self.node_id_table.contains_key(&destination) {
            NodeIdTable::update(self, entry)?;
        } else {
            NodeIdTable::create(self, entry)?;
        }
        Ok(())
    }
}

impl PathIdTable for InMemoryFwdTables {
    type Error = error::FwdTableError;

    fn create(&mut self, entry: PathIdEntry) -> Result<(), Self::Error> {
        let in_path_id = match entry {
            PathIdEntry::Decapsulate(ref entry) => entry.in_path_id.clone(),
            PathIdEntry::Forward(ref entry) => entry.in_path_id.clone(),
        };

        if self.path_id_table.contains_key(&in_path_id) {
            return Err(error::FwdTableError::EntryAlreadyExists);
        }

        log::trace!(target: "in_memory_fwd_table", "Created {:?}", entry);

        self.path_id_table.insert(in_path_id, entry);

        Ok(())
    }

    fn update(&mut self, entry: PathIdEntry) -> Result<(), Self::Error> {
        let in_path_id = match entry {
            PathIdEntry::Decapsulate(ref entry) => entry.in_path_id.clone(),
            PathIdEntry::Forward(ref entry) => entry.in_path_id.clone(),
        };

        if let Some(old_entry) = self.path_id_table.get_mut(&in_path_id) {
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
    
    fn create_or_update(&mut self, entry: PathIdEntry) -> Result<(), Self::Error> {
        let in_path_id = match entry {
            PathIdEntry::Decapsulate(ref entry) => entry.in_path_id.clone(),
            PathIdEntry::Forward(ref entry) => entry.in_path_id.clone(),
        };

        if self.path_id_table.contains_key(&in_path_id) {
            PathIdTable::update(self, entry)
        } else {
            PathIdTable::create(self, entry)
        }
    }
}

impl ForwardingTables for InMemoryFwdTables {}

impl Extend<NodeIdEntry> for InMemoryFwdTables {
    fn extend<T: IntoIterator<Item = NodeIdEntry>>(&mut self, iter: T) {
        let entries = iter
            .into_iter()
            .map(|entry| {
                let destination = match entry {
                    NodeIdEntry::Forward(ref entry) => entry.destination.clone(),
                    NodeIdEntry::Encapsulate(ref entry) => entry.destination.clone(),
                };
                (destination, entry)});
        self.node_id_table.extend(entries);
    }
}

impl Extend<PathIdEntry> for InMemoryFwdTables {
    fn extend<T: IntoIterator<Item = PathIdEntry>>(&mut self, iter: T) {
        let entries = iter
            .into_iter()
            .map(|entry| {
                let in_path_id = match entry {
                    PathIdEntry::Decapsulate(ref entry) => entry.in_path_id.clone(),
                    PathIdEntry::Forward(ref entry) => entry.in_path_id.clone(),
                };
                (in_path_id, entry)});
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
