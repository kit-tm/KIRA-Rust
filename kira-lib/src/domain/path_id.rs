use std::fmt::{Debug, Display, Formatter, LowerHex, UpperHex};
use std::net::Ipv6Addr;
use std::str::FromStr;

use digest::Digest;

#[cfg(feature = "sha2")]
pub use sha2_extension::*;
#[cfg(feature = "sha3")]
pub use sha3_extension::*;

use crate::domain::{NodeId, SIZE};

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
            hasher.update(id.as_ref());
        }
        Self::from(hasher.finalize().to_vec())
    }
}

impl PathId {
    pub fn from_sha1<'a, I: IntoIterator<Item = &'a NodeId>>(path: I) -> Self {
        Self::from_digest(sha1::Sha1::new(), path)
    }
}

#[cfg(feature = "sha2")]
mod sha2_extension {
    use sha2::Digest;

    use crate::domain::path_id::PathId;
    use crate::domain::NodeId;

    impl PathId {
        pub fn from_sha2_256<'a, I: IntoIterator<Item = &'a NodeId>>(path: I) -> Self {
            Self::from_digest(sha2::Sha256::new(), path)
        }
    }

    impl PathId {
        pub fn from_sha2_512<'a, I: IntoIterator<Item = &'a NodeId>>(path: I) -> Self {
            Self::from_digest(sha2::Sha512::new(), path)
        }
    }
}

#[cfg(feature = "sha3")]
mod sha3_extension {
    use sha3::Digest;

    use crate::domain::path_id::PathId;
    use crate::domain::NodeId;

    impl PathId {
        pub fn from_sha3_256<'a, I: IntoIterator<Item = &'a NodeId>>(path: I) -> Self {
            Self::from_digest(sha3::Sha3_256::new(), path)
        }
    }

    impl PathId {
        pub fn from_sha3_512<'a, I: IntoIterator<Item = &'a NodeId>>(path: I) -> Self {
            Self::from_digest(sha3::Sha3_512::new(), path)
        }
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
        bytes[0] = 0xfc;
        bytes[1] = 0xaa;
        bytes[2..16].copy_from_slice(&value.bytes[0..14]);
        Ipv6Addr::from(bytes)
    }
}

impl FromStr for PathId {
    type Err = hex::FromHexError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // fill in zeros if '::' at the end
        let bytes_string = s.as_bytes();
        let len = bytes_string.len();
        let bytes = if bytes_string[len - 1] as char == ':' && bytes_string[len - 2] as char == ':'
        {
            let mut bytes = hex::decode(&bytes_string[..len - 2])?;
            bytes.resize(SIZE, 0u8);
            bytes
        } else {
            hex::decode(bytes_string)?
        };

        Ok(Self { bytes })
    }
}

// ============ Output Formatters ============

impl LowerHex for PathId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match (f.precision(), f.alternate()) {
            (Some(precision), _) => write!(f, "{}", &hex::encode(self)[..precision]),
            (None, true) => write!(f, "{}", &hex::encode(self)[..SHORT_OUTPUT_LENGTH]),
            (None, false) => write!(f, "{}", hex::encode(self)),
        }
    }
}

impl UpperHex for PathId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match (f.precision(), f.alternate()) {
            (Some(precision), _) => write!(f, "{}", &hex::encode_upper(self)[..precision]),
            (None, true) => write!(f, "{}", &hex::encode_upper(self)[..SHORT_OUTPUT_LENGTH]),
            (None, false) => write!(f, "{}", hex::encode_upper(self)),
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
        UpperHex::fmt(self, f)
    }
}
