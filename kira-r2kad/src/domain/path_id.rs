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

#[cfg(test)]
mod tests {
    use std::{
        net::Ipv6Addr,
        str::FromStr,
    };

    use super::*;
    use crate::domain::NodeId;

    /// SHA-1 over the 14-byte big-endian representation of NodeId(1) and NodeId(2).
    const DIGEST_1_2: &str = "1ea54dacee661245cdfaf6b442ee80817504de33";
    /// SHA-1 over the 14-byte big-endian representation of NodeId(7).
    const DIGEST_7: &str = "e6079d754285b55566367aaa56398c1d085d16c9";
    /// SHA-1 over the empty input.
    const DIGEST_EMPTY: &str = "da39a3ee5e6b4b0d3255bfef95601890afd80709";

    #[test]
    fn sha1_matches_reference_digest() {
        let path_id = PathId::from_sha1(&[NodeId::from(1), NodeId::from(2)]);
        assert_eq!(hex::encode(path_id.as_ref()), DIGEST_1_2);
    }

    #[test]
    fn sha1_single_hop() {
        let path_id = PathId::from_sha1(&[NodeId::from(7)]);
        assert_eq!(hex::encode(path_id.as_ref()), DIGEST_7);
    }

    #[test]
    fn sha1_empty_path() {
        let path: &[NodeId] = &[];
        let path_id = PathId::from_sha1(path);
        assert_eq!(hex::encode(path_id.as_ref()), DIGEST_EMPTY);
    }

    #[test]
    fn sha1_is_order_sensitive() {
        let forward = PathId::from_sha1(&[NodeId::from(1), NodeId::from(2)]);
        let backward = PathId::from_sha1(&[NodeId::from(2), NodeId::from(1)]);
        assert_ne!(forward, backward);
    }

    #[test]
    fn sha1_output_is_20_bytes() {
        let path_id = PathId::from_sha1(&[NodeId::from(1)]);
        assert_eq!(path_id.as_ref().len(), 20);
    }

    #[test]
    fn ipv6_conversion_uses_prefix_and_first_14_bytes() {
        let path_id = PathId::from_sha1(&[NodeId::from(1), NodeId::from(2)]);
        let addr = Ipv6Addr::from(&path_id);

        // prefix bytes fc/aa followed by the first 14 digest bytes
        let digest = hex::decode(DIGEST_1_2).unwrap();
        let mut expected = [0u8; 16];
        expected[0] = 0xfc;
        expected[1] = 0xaa;
        expected[2..16].copy_from_slice(&digest[0..14]);
        assert_eq!(addr, Ipv6Addr::from(expected));
    }

    #[test]
    fn from_str_round_trip() {
        let path_id = PathId::from_sha1(&[NodeId::from(1), NodeId::from(2)]);
        let parsed = PathId::from_str(&hex::encode(path_id.as_ref())).unwrap();
        assert_eq!(parsed, path_id);
    }

    #[test]
    fn from_str_rejects_invalid_hex() {
        assert!(PathId::from_str("zzzz").is_err());
        assert!(PathId::from_str("abc").is_err(), "odd number of digits");
    }

    #[test]
    fn bitxor_takes_first_node_id_sized_chunk() {
        let bytes: Vec<u8> = (0u8..=15).collect();
        let path_id = PathId::from(bytes.clone());
        let other = NodeId::from(1);

        let result = path_id ^ &other;

        let mut prefix = [0u8; NodeId::SIZE];
        prefix.copy_from_slice(&bytes[..NodeId::SIZE]);
        assert_eq!(result, NodeId::from(prefix) ^ other);
    }

    #[test]
    fn upper_hex_default_truncates_to_28_chars() {
        let path_id = PathId::from_sha1(&[NodeId::from(1), NodeId::from(2)]);
        assert_eq!(format!("{path_id}"), "1EA54DACEE661245CDFAF6B442EE");
    }

    #[test]
    fn lower_hex_default_truncates_to_28_chars() {
        let path_id = PathId::from_sha1(&[NodeId::from(1), NodeId::from(2)]);
        assert_eq!(format!("{path_id:x}"), "1ea54dacee661245cdfaf6b442ee");
    }

    #[test]
    fn alternate_lower_hex_truncates_to_short_output_length() {
        let path_id = PathId::from_sha1(&[NodeId::from(1), NodeId::from(2)]);
        assert_eq!(format!("{path_id:#x}"), "1ea54dac");
        assert_eq!(format!("{path_id:#X}"), "1EA54DAC");
    }

    #[test]
    fn precision_limits_output_length() {
        let path_id = PathId::from_sha1(&[NodeId::from(1), NodeId::from(2)]);
        assert_eq!(format!("{path_id:.4}"), "1EA5");
        assert_eq!(format!("{path_id:.4x}"), "1ea5");
    }

    #[test]
    fn debug_wraps_upper_hex() {
        let path_id = PathId::from_sha1(&[NodeId::from(1), NodeId::from(2)]);
        assert_eq!(format!("{path_id:?}"), "PathId(1EA54DACEE661245CDFAF6B442EE)");
    }
}
