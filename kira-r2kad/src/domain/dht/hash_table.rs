//! Abstractions of the local hash table.

use std::error::Error;
use std::fmt::Display;
use std::sync::Arc;
use std::time::Instant;

pub use complex_hash_table::ComplexHashTable;
pub use single_value_hash_table::SingleValueHashTable;
pub mod complex_hash_table;

use crate::domain::NodeId;

pub mod single_value_hash_table;

/// Abstraction of the local hash table of the node.
///
/// Nodes use a local hash table to store key-value pairs of the
/// network-provided distributed hash table.
///
/// The [`SingleValueHashTable`] is a simple sample implementation.
pub trait LocalHashTable {
    type StoreOk: Display;
    type StoreErr: Error;
    type FetchErr: Error;

    /// Stores a key-value pair in the hash table.
    fn store(&mut self, key: NodeId, value: Arc<[u8]>) -> Result<Self::StoreOk, Self::StoreErr>;

    /// Returns the values stored at a key in the hash table.
    fn fetch(&self, key: &NodeId) -> Result<impl Iterator<Item = Arc<[u8]>>, Self::FetchErr>;

    /// Returns all entries of the hash table.
    fn fetch_all(&self) -> impl Iterator<Item = (&NodeId, impl Iterator<Item = Arc<[u8]>>)>;

    /// Removes al values stored at a key in the hash table.
    ///
    /// Returns if values were stored at the key.
    fn remove(&mut self, key: &NodeId) -> bool;

    /// Returns the metadata of the entry in the local hash table.
    ///
    /// Returns [`None`] if the key has no associated value because
    /// the key is not stored in the hash table.
    fn meta(&self, key: &NodeId) -> Option<&impl EntryMeta>;

    /// Returns the metadata of the entry in the local hash table.
    ///
    /// Returns [`None`] if the key has no associated value because
    /// the key is not stored in the hash table.
    fn meta_mut(&mut self, key: &NodeId) -> Option<&mut impl EntryMeta>;
}

/// Metadata about entries of the [LocalHashTable].
///
/// An _entry_ is a key with potentially multiple associated values in the [LocalHashTable].
/// The entry is comprised out of multiple key-value pairs with the same key.
///
/// A hash tables only tracks the metadata of entries because it is not possible
/// to access individual key-value pairs but only keys.
pub trait EntryMeta {
    /// Returns the last access time of a entry in the hash table.
    ///
    /// Returns [`None`] if a entry was never accessed.
    fn last_access(&self) -> Option<Instant>;

    /// Updates the access time of an entry.
    ///
    /// The access time of an entry can't be updated to the past.
    /// Returns if the access time was successfully updated.
    fn access(&mut self, now: Instant) -> bool;

    /// Returns the last republish time of a entry in the hash table.
    ///
    /// Returns [`None`] if a entry was never republished.
    fn last_republish(&self) -> Option<Instant>;

    /// Updates the republish time of an entry.
    ///
    /// The republish time of an entry can't be updated to the past.
    /// Returns if the republish time was successfully updated.
    fn republished(&mut self, now: Instant) -> bool;
}
