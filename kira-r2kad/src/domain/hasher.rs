use std::fmt::Debug;

use crate::domain::{
    NodeId,
    path_id::PathId,
};

/// Enum collecting all supported hash algorithms for configuration of use cases which calculate
/// [PathId]s from a series of [NodeId]s.
///
/// Default is **SHAKE-256**.
/// **sha2** and **sha3** variants can be enabled through the feature flags `sha2, sha3`.
#[derive(Debug, Default, Eq, PartialEq, Copy, Clone)]
pub enum Hasher {
    #[default]
    Shake256,
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
            Hasher::Shake256 => PathId::from_shake256(path),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::PathId;

    #[test]
    fn default_is_shake256() {
        assert_eq!(Hasher::default(), Hasher::Shake256);
    }

    #[test]
    fn shake256_matches_path_id_helper() {
        let path = [NodeId::from(1), NodeId::from(2)];
        assert_eq!(Hasher::Shake256.hash(&path), PathId::from_shake256(&path));
    }

    #[cfg(feature = "sha2")]
    #[test]
    fn sha2_output_lengths() {
        let path = [NodeId::from(1), NodeId::from(2)];
        assert_eq!(Hasher::Sha2_256.hash(&path).as_ref().len(), 32);
        assert_eq!(Hasher::Sha2_512.hash(&path).as_ref().len(), 64);
    }

    #[cfg(feature = "sha3")]
    #[test]
    fn sha3_output_lengths() {
        let path = [NodeId::from(1), NodeId::from(2)];
        assert_eq!(Hasher::Sha3_256.hash(&path).as_ref().len(), 32);
        assert_eq!(Hasher::Sha3_512.hash(&path).as_ref().len(), 64);
    }
}
