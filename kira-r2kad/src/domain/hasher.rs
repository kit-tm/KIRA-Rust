use crate::domain::path_id::PathId;
use crate::domain::NodeId;
use std::fmt::Debug;

/// Enum collecting all supported hash algorithms for configuration of use cases which calculate
/// [PathId]s from a series of [NodeId]s.
///
/// Default is **sha1**.
/// **sha2** and **sha3** variants can be enabled through the feature flags `sha2, sha3`.
#[derive(Debug, Default, Eq, PartialEq, Copy, Clone)]
pub enum Hasher {
    #[default]
    Sha1,
    #[cfg(feature = "sha2")]
    Sha2_256,
    #[cfg(feature = "sha2")]
    Sha2_512,
    #[cfg(feature = "sha3")]
    Sha3_256,
    #[cfg(feature = "sha3")]
    Sha3_512,
}

impl Hasher {
    /// Calculates the [PathId] for an iterator of [NodeId]s with the related hash for the [Hasher]
    /// variant.
    pub fn hash<'a>(&self, path: impl IntoIterator<Item = &'a NodeId>) -> PathId {
        match self {
            Hasher::Sha1 => PathId::from_sha1(path),
            #[cfg(feature = "sha2")]
            Hasher::Sha2_256 => PathId::from_sha2_256(path),
            #[cfg(feature = "sha2")]
            Hasher::Sha2_512 => PathId::from_sha2_512(path),
            #[cfg(feature = "sha3")]
            Hasher::Sha3_256 => PathId::from_sha3_256(path),
            #[cfg(feature = "sha3")]
            Hasher::Sha3_512 => PathId::from_sha3_512(path),
        }
    }
}
