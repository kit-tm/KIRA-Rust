use std::{
    fmt::{
        Debug,
        Display,
        Formatter,
        LowerHex,
        UpperHex,
    },
    str::FromStr,
};

use digest::{
    Digest,
    ExtendableOutput,
    Update,
};
use tracing::Level;

use crate::domain::NodeId;

pub const SHORT_OUTPUT_LENGTH: usize = 8;

/// Number of bytes squeezed from SHAKE-256 when generating a [PathId].
pub const SHAKE256_OUTPUT_LENGTH: usize = 32;

/// [PathId] uniquely identifies a [Path](crate::domain::path::Path).
///
/// Uses SHAKE-256 (default), SHA-2 or SHA-3 for hash generation.
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

    /// Calculates the [PathId] with SHAKE-256, producing [SHAKE256_OUTPUT_LENGTH] bytes.
    pub fn from_shake256<'a, I: IntoIterator<Item = &'a NodeId>>(path: I) -> Self {
        if tracing::enabled!(target: "path_id", Level::TRACE) {
            let path = path.into_iter().collect::<Vec<_>>();
            let path_str = format!("{path:?}");
            let result = Self::shake256(path);
            tracing::trace!(target: "path_id", path=path_str, path_id=format!("{:X}",result), "Calculated path_id");
            result
        } else {
            Self::shake256(path)
        }
    }

    fn shake256<'a, I: IntoIterator<Item = &'a NodeId>>(path: I) -> Self {
        let mut hasher = shake::Shake256::default();
        for id in path {
            hasher.update(&id.to_be_bytes());
        }
        let mut bytes = vec![0u8; SHAKE256_OUTPUT_LENGTH];
        hasher.finalize_xof_into(&mut bytes);
        Self::from(bytes)
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

    /// SHAKE-256 (32 bytes) over the 14-byte big-endian representation of NodeId(1) and NodeId(2).
    const DIGEST_1_2: &str = "9b5612eaeadc93dddf7e5d58f827a33067974caf32aa353e956e8acdcf12f703";
    /// SHAKE-256 (32 bytes) over the 14-byte big-endian representation of NodeId(7).
    const DIGEST_7: &str = "99d5a44fbaf20cb649bb959daebfb56d3bb9fd3ab2cf7c09e02a6b465f22633f";
    /// SHAKE-256 (32 bytes) over the empty input.
    const DIGEST_EMPTY: &str = "46b9dd2b0ba88d13233b3feb743eeb243fcd52ea62b81b82b50c27646ed5762f";

    #[test]
    fn shake256_matches_reference_digest() {
        let path_id = PathId::from_shake256(&[NodeId::from(1), NodeId::from(2)]);
        assert_eq!(hex::encode(path_id.as_ref()), DIGEST_1_2);
    }

    #[test]
    fn shake256_single_hop() {
        let path_id = PathId::from_shake256(&[NodeId::from(7)]);
        assert_eq!(hex::encode(path_id.as_ref()), DIGEST_7);
    }

    #[test]
    fn shake256_empty_path() {
        let path: &[NodeId] = &[];
        let path_id = PathId::from_shake256(path);
        assert_eq!(hex::encode(path_id.as_ref()), DIGEST_EMPTY);
    }

    #[test]
    fn shake256_is_order_sensitive() {
        let forward = PathId::from_shake256(&[NodeId::from(1), NodeId::from(2)]);
        let backward = PathId::from_shake256(&[NodeId::from(2), NodeId::from(1)]);
        assert_ne!(forward, backward);
    }

    #[test]
    fn shake256_output_is_32_bytes() {
        let path_id = PathId::from_shake256(&[NodeId::from(1)]);
        assert_eq!(path_id.as_ref().len(), SHAKE256_OUTPUT_LENGTH);
    }

    #[test]
    fn ipv6_conversion_uses_prefix_and_first_14_bytes() {
        let path_id = PathId::from_shake256(&[NodeId::from(1), NodeId::from(2)]);
        let addr = crate::domain::IpPrefixes::default().path_ip(&path_id);

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
        let path_id = PathId::from_shake256(&[NodeId::from(1), NodeId::from(2)]);
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
        let path_id = PathId::from_shake256(&[NodeId::from(1), NodeId::from(2)]);
        assert_eq!(format!("{path_id}"), "9B5612EAEADC93DDDF7E5D58F827");
    }

    #[test]
    fn lower_hex_default_truncates_to_28_chars() {
        let path_id = PathId::from_shake256(&[NodeId::from(1), NodeId::from(2)]);
        assert_eq!(format!("{path_id:x}"), "9b5612eaeadc93dddf7e5d58f827");
    }

    #[test]
    fn alternate_lower_hex_truncates_to_short_output_length() {
        let path_id = PathId::from_shake256(&[NodeId::from(1), NodeId::from(2)]);
        assert_eq!(format!("{path_id:#x}"), "9b5612ea");
        assert_eq!(format!("{path_id:#X}"), "9B5612EA");
    }

    #[test]
    fn precision_limits_output_length() {
        let path_id = PathId::from_shake256(&[NodeId::from(1), NodeId::from(2)]);
        assert_eq!(format!("{path_id:.4}"), "9B56");
        assert_eq!(format!("{path_id:.4x}"), "9b56");
    }

    #[test]
    fn debug_wraps_upper_hex() {
        let path_id = PathId::from_shake256(&[NodeId::from(1), NodeId::from(2)]);
        assert_eq!(
            format!("{path_id:?}"),
            "PathId(9B5612EAEADC93DDDF7E5D58F827)"
        );
    }
}
