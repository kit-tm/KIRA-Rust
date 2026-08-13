use std::{
    fmt::{
        Debug,
        Display,
        Formatter,
        LowerHex,
        UpperHex,
    },
    net::Ipv6Addr,
    str::FromStr,
};

use digest::Digest;
use tracing::Level;

use crate::domain::NodeId;

pub const SHORT_OUTPUT_LENGTH: usize = 8;

/// [PathId] uniquely identifies a [Path](crate::domain::path::Path).
///
/// Uses SHA-1, SHA-2 or SHA-3 for hash generation.
/// The byte length of the [PathId] is not fixed and depends on the used hash algorithm.
#[derive(Eq, PartialEq, Hash, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct PathId {
    bytes: Vec<u8>,
}

impl From<Vec<u8>> for PathId {
    fn from(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }
}

impl PathId {
    pub fn from_digest<'a, D: Digest, I: IntoIterator<Item = &'a NodeId>>(
        mut hasher: D,
        path: I,
    ) -> Self {
        for id in path {
            let id_in_bytes: [u8; NodeId::SIZE] = id.to_be_bytes();
            hasher.update(id_in_bytes);
        }
        Self::from(hasher.finalize().to_vec())
    }

    pub fn from_sha1<'a, I: IntoIterator<Item = &'a NodeId>>(path: I) -> Self {
        if tracing::enabled!(target: "path_id", Level::TRACE) {
            let path = path.into_iter().collect::<Vec<_>>();
            let path_str = format!("{path:?}");
            let result = Self::from_digest(sha1::Sha1::new(), path);
            tracing::trace!(target: "path_id", path=path_str, path_id=format!("{:X}",result), "Calculated path_id");
            result
        } else {
            Self::from_digest(sha1::Sha1::new(), path)
        }
    }

    #[cfg(feature = "sha2")]
    pub fn from_sha2_256<'a, I: IntoIterator<Item = &'a NodeId>>(path: I) -> Self {
        Self::from_digest(sha2::Sha256::new(), path)
    }

    #[cfg(feature = "sha2")]
    pub fn from_sha2_512<'a, I: IntoIterator<Item = &'a NodeId>>(path: I) -> Self {
        Self::from_digest(sha2::Sha512::new(), path)
    }

    #[cfg(feature = "sha3")]
    pub fn from_sha3_256<'a, I: IntoIterator<Item = &'a NodeId>>(path: I) -> Self {
        Self::from_digest(sha3::Sha3_256::new(), path)
    }

    #[cfg(feature = "sha3")]
    pub fn from_sha3_512<'a, I: IntoIterator<Item = &'a NodeId>>(path: I) -> Self {
        Self::from_digest(sha3::Sha3_512::new(), path)
    }
}

impl AsRef<[u8]> for PathId {
    fn as_ref(&self) -> &[u8] {
        self.bytes.as_ref()
    }
}

impl From<&PathId> for Ipv6Addr {
    fn from(value: &PathId) -> Self {
        let mut bytes = [0; 16];
        bytes[0] = 0xfc; // TODO prefix must be configurable
        bytes[1] = 0xaa; // TODO prefix must be configurable
        bytes[2..16].copy_from_slice(&value.bytes[0..14]);
        Ipv6Addr::from(bytes)
    }
}

impl FromStr for PathId {
    type Err = hex::FromHexError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = hex::decode(s)?;
        Ok(Self { bytes })
    }
}

impl std::ops::BitXor<&NodeId> for PathId {
    type Output = NodeId;

    fn bitxor(self, other: &NodeId) -> NodeId {
        let mut tmp_bytes = [0u8; NodeId::SIZE];
        tmp_bytes.copy_from_slice(&self.bytes[..NodeId::SIZE]);
        NodeId::from(tmp_bytes) ^ *other
    }
}

// ============ Output Formatters ============

impl LowerHex for PathId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match (f.precision(), f.alternate()) {
            (Some(precision), _) => write!(f, "{}", &hex::encode(self)[..precision]),
            (None, true) => write!(f, "{}", &hex::encode(self)[..SHORT_OUTPUT_LENGTH]),
            (None, false) => write!(f, "{}", &hex::encode(self)[..28]),
        }
    }
}

impl UpperHex for PathId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match (f.precision(), f.alternate()) {
            (Some(precision), _) => write!(f, "{}", &hex::encode_upper(self)[..precision]),
            (None, true) => write!(f, "{}", &hex::encode_upper(self)[..SHORT_OUTPUT_LENGTH]),
            (None, false) => write!(f, "{}", &hex::encode_upper(self)[..28]),
        }
    }
}

impl Display for PathId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        UpperHex::fmt(self, f)
    }
}

impl Debug for PathId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "PathId({self:X})")
    }
}
