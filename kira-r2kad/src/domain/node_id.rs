use rand::Rng;
use serde;
use serde::de::{self, Visitor};
use std::cmp::Ordering;
use std::fmt;
use std::fmt::{Debug, Formatter, LowerHex, UpperHex};
use std::net::Ipv6Addr;
use std::num::NonZeroU8;
use std::ops::BitXor;
use std::str::FromStr;

use derive_more::with_trait::{Display, Error};

/// A NodeId with default SIZE of 112 Bits (14 Byte) as default value as proposed in the Internet-Draft (protocol specification).
///
/// This implementation supports creating NodeIds with a given byte size.
///
/// As all NodeIds have to be of the same size for an application this implementation
/// uses const generics to specify its size instead of using [Vec] (which uses Heap
/// allocation by default).
#[derive(Clone, Copy, Eq, PartialEq, Hash, Display, PartialOrd, Ord)]
#[display("{self:x}")]
pub struct NodeId {
    node_id: u128,
}

// ============ Initializers ============

/// Constant initializers and functions related to them.
impl NodeId {
    /// The byte size of a [NodeId].
    ///
    /// The length of the [NodeId] is a global parameter not to be
    /// altered among nodes unlike the bucket size (**k**).
    pub const SIZE: usize = 14;
    /// The size of a [NodeId] in bits.
    pub const BITS: u8 = 112;
    /// Length in Characters of the short output format for [NodeId]s.
    /// const SHORT_OUTPUT_LENGTH: usize = 8;
    /// Maximum value and mask to set the highest 16 bit to 0
    const MAX_UVAL: u128 = 0x0000ffff_ffffffff_ffffffff_ffffffff;
    /// undefined is all zeros
    const UNDEFINED_UVAL: u128 = 0u128;
    const ALL_NODES_UVAL: u128 = NodeId::MAX_UVAL;

    /// A [NodeId] with the numerical value of 0.
    pub const ZERO: Self = Self { node_id: 0u128 };

    /// A [NodeId] with the undefined value.
    pub const UNDEFINED: Self = Self {
        node_id: Self::UNDEFINED_UVAL,
    };

    pub const ALL_NODES: Self = Self {
        node_id: Self::ALL_NODES_UVAL,
    };

    /// [NodeId] with the numerical value of 1.
    pub const ONE: Self = Self { node_id: 1u128 };

    /// The maximum representable [NodeId] for the given Byte size.
    ///
    /// This is equal to all bits in a [NodeId] == 1.
    pub const MAX: Self = Self {
        node_id: Self::MAX_UVAL,
    };

    /// Creates a random [NodeId].
    pub fn random() -> Self {
        // generated [NodeId] is guaranteed to not be equal to [NodeId::UNDEFINED] or [NodeId::ALL_NODES].
        let mut inner = 0u128;
        let mut myrng = rand::rng();
        while inner == Self::UNDEFINED_UVAL || inner == Self::ALL_NODES_UVAL {
            inner = myrng.random::<u128>();
        }
        Self {
            node_id: inner & Self::MAX_UVAL,
        }
    }

    /// Creates an all zeroed-out id with the LSB set to the given value.
    pub fn with_lsb(lsb: u8) -> Self {
        Self {
            node_id: u128::from(lsb),
        }
    }

    /// Creates an all zeroed-out id with the MSB set to the given value.
    pub fn with_msb(msb: u8) -> Self {
        Self {
            node_id: u128::from(msb) << (NodeId::BITS - 8),
        }
    }

    /// Checks if the [NodeId] is equal to the numerical value of 0;
    #[cfg(test)]
    fn is_zero(&self) -> bool {
        self.node_id == 0u128
    }

    /// Checks if the [NodeId] is equal to the numerical value of 1;
    #[cfg(test)]
    fn is_one(&self) -> bool {
        self.node_id == 1u128
    }

    pub fn leading_zeros(&self) -> u8 {
        const IGNORE_BITS: u32 = u128::BITS - (NodeId::BITS as u32);
        // this is carried out on the full u128!
        let mut lz = self.node_id.leading_zeros();
        if lz >= IGNORE_BITS {
            lz -= IGNORE_BITS;
            lz.try_into().unwrap()
        } else {
            panic!("NodeId internal error: leading zeros value invalid ({lz})");
        }
    }

    /// Returns the shared prefix length in number of bits
    pub fn shared_prefix_bits(&self, other: &Self) -> SharedPrefix {
        self.shared_prefix_len(other, NonZeroU8::MIN)
            .expect("group size of one is always valid")
    }

    /// Returns the shared prefix length in number of groups of bits that match
    ///
    /// # Notes
    ///
    /// This algorithm uses basic operations for computing the shared prefix.
    /// As this function is used every time a packet arrives optimizing it may be
    /// worth the effort.
    pub fn shared_prefix_len(
        &self,
        other: &Self,
        bits_per_group: NonZeroU8,
    ) -> Result<SharedPrefix, GroupingError> {
        if bits_per_group.get() > Self::BITS {
            return Err(GroupingError::Invalid {
                group_size: bits_per_group,
            });
        }
        let bits_per_group = bits_per_group.get();

        let xor: Self = self ^ other;

        if self == other {
            return Ok(SharedPrefix {
                xor,
                length: Self::BITS / bits_per_group,
            });
        }

        Ok(SharedPrefix {
            xor,
            length: xor.leading_zeros() / bits_per_group,
        })
    }

    /// Returns the bit at the index starting from LSB. Returns either 0 or 1.
    pub fn bit(&self, bit_index: u8) -> Result<u8, BitIndexOutOfBounds> {
        if bit_index >= Self::BITS {
            return Err(BitIndexOutOfBounds);
        }

        let result = self.node_id & (1u128 << bit_index);
        if result == 0 { Ok(0) } else { Ok(1) }
    }

    /// Returns a number representing the bits from an inclusive position from LSB to MSB.
    pub fn bits(
        &self,
        from_bit_index: u8,
        num_bits: NonZeroU8,
    ) -> Result<u128, BitIndexOutOfBounds> {
        let num_bits = num_bits.get();
        if from_bit_index + num_bits > Self::BITS {
            return Err(BitIndexOutOfBounds);
        }

        // create bit mask with zeros right and left
        let mut mask = Self::MAX_UVAL >> from_bit_index;
        mask <<= from_bit_index;
        mask <<= u128::BITS - (from_bit_index + num_bits) as u32;
        mask >>= u128::BITS - (from_bit_index + num_bits) as u32;

        Ok((self.node_id & mask) >> from_bit_index)
    }

    /// Returns the prefix of the given length with the rest set to 0
    pub fn prefix(&self, prefix_len: u8) -> Self {
        let mut mask = Self::MAX_UVAL >> (Self::BITS - prefix_len) as u128;
        mask <<= (Self::BITS - prefix_len) as u128;

        log::trace!(target: "node_id", "prefix with length {} of {:?} is {:?}", prefix_len, self, NodeId { node_id : self.node_id & mask });

        NodeId {
            node_id: self.node_id & mask,
        }
    }

    // outputs array of bytes in network byte order
    pub fn to_be_bytes(&self) -> [u8; NodeId::SIZE] {
        let mut output = [0u8; NodeId::SIZE];
        output.copy_from_slice(&self.node_id.to_be_bytes()[2..]);
        output
    }
}

impl serde::Serialize for NodeId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let first_non_zero_byte = self.node_id.leading_zeros() as usize / 8;
        let slice = &self.node_id.to_be_bytes()[first_non_zero_byte..];
        serializer.serialize_bytes(slice)
    }
}

struct NodeIdVisitor;

impl<'de> Visitor<'de> for NodeIdVisitor {
    type Value = NodeId;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a NodeId as byte array")
    }

    // fn visit_u128<E>(self, v: u128) -> Result<Self::Value, E>
    // where
    //     E: de::Error, {
    //     Ok(NodeId { node_id : v })
    // }

    fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if v.len() <= NodeId::SIZE {
            let mut output = [0u8; size_of::<u128>()];
            for (i, b) in v[..v.len()].iter().enumerate() {
                output[size_of::<u128>() - v.len() + i] = *b;
            }
            Ok(NodeId {
                node_id: u128::from_be_bytes(output),
            })
        } else {
            Err(E::custom(format!(
                "NodeId longer than {} bytes",
                NodeId::SIZE
            )))
        }
    }

    fn visit_borrowed_bytes<E>(self, v: &'de [u8]) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        if v.len() <= NodeId::SIZE {
            let mut output = [0u8; size_of::<u128>()];
            for (i, b) in v[..v.len()].iter().enumerate() {
                output[size_of::<u128>() - v.len() + i] = *b;
            }
            Ok(NodeId {
                node_id: u128::from_be_bytes(output),
            })
        } else {
            Err(E::custom(format!(
                "NodeId longer than {} bytes",
                NodeId::SIZE
            )))
        }
    }
}

impl<'de> serde::Deserialize<'de> for NodeId {
    fn deserialize<D>(deserializer: D) -> Result<NodeId, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_bytes(NodeIdVisitor)
    }
}

#[derive(Debug, Eq, PartialEq, Display, Error)]
#[display("Bit Index out of bounds")]
pub struct BitIndexOutOfBounds;

/// A [NodeId] subnet with a given prefix length.
#[derive(Debug, Eq, PartialEq, Clone, Hash, Display)]
#[display("{node_id}/{prefix_length}")]
pub struct NodeIdSubnet {
    node_id: NodeId,
    prefix_length: u8,
}

#[derive(Debug, Copy, Clone, Display, Error)]
#[display("Prefix length is larger then bits of NodeId")]
pub struct PrefixLengthError;

impl NodeIdSubnet {
    /// Creates a new [NodeIdSubnet].
    ///
    /// This function return [PrefixLengthError] if the `prefix_length`
    /// is larger then [NodeId::BITS].
    pub fn try_new(node_id: NodeId, prefix_length: u8) -> Result<NodeIdSubnet, PrefixLengthError> {
        if prefix_length <= NodeId::BITS {
            Ok(Self {
                node_id,
                prefix_length,
            })
        } else {
            Err(PrefixLengthError)
        }
    }

    /// Creates new [NodeIdSubnet] with a zero-sized prefix length.
    pub fn new(node_id: NodeId) -> Self {
        Self {
            node_id,
            prefix_length: 0,
        }
    }

    /// Returns the prefix length.
    ///
    /// The prefix length is in between zero and [NodeId::BITS].
    pub const fn prefix_length(&self) -> u8 {
        self.prefix_length
    }

    /// Returns the node_id.
    pub const fn node_id(&self) -> &NodeId {
        &self.node_id
    }

    /// Returns the [Ipv6Addr] subnet equivalent representation of this [NodeIdSubnet].
    ///
    /// The second parameter is the [prefix length](Self::ipv6_subnet_prefix_length)
    pub fn to_ipv6_subnet(&self) -> (Ipv6Addr, u8) {
        let prefix_len = self.ipv6_subnet_prefix_length();
        ((&self.node_id).into(), prefix_len)
    }

    /// Returns a valid [Ipv6Addr] subnet prefix length.
    pub const fn ipv6_subnet_prefix_length(&self) -> u8 {
        const IPV6_BITS: u8 = Ipv6Addr::BITS as u8;
        const COMMON_BITS: u8 = IPV6_BITS - NodeId::BITS;

        if self.prefix_length() == 0 {
            IPV6_BITS
        } else {
            COMMON_BITS + self.prefix_length()
        }
    }
}

/// Shared prefix of two [NodeId]s.
///
/// Contains the shared prefix length in number of bits and the computed
/// XOR [NodeId] of the two origin [NodeId]s.
///
/// # Ordering
///
/// Consider the scenario where for nodes A and B two [SharedPrefix]es **a**, **b** are calculated
/// for the same target [NodeId] **X**.
/// The [SharedPrefix]es **a** and **b** are ordered as followed:
///
/// > a < b: a is closer to **X** than b => Shared prefix is longer **or** ( shared prefix has
/// > equal length **and** numerical value of xor is smaller )
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct SharedPrefix {
    pub(crate) xor: NodeId,
    pub(crate) length: u8,
}

impl From<SharedPrefix> for u8 {
    fn from(prefix: SharedPrefix) -> Self {
        prefix.length
    }
}

impl SharedPrefix {
    pub fn into_xor(self) -> NodeId {
        self.xor
    }

    pub fn xor(&self) -> &NodeId {
        &self.xor
    }

    pub fn bit_len(&self) -> u8 {
        self.length
    }

    pub fn into_bit_len(self) -> u8 {
        self.length
    }
}

impl PartialOrd for SharedPrefix {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

// Ordering::Less => Closer = Longer matching prefix
impl Ord for SharedPrefix {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self.length.cmp(&other.length), self.xor.cmp(&other.xor)) {
            (Ordering::Equal, xor_ordering) => xor_ordering,
            (ordering, _) => ordering.reverse(),
        }
    }
}

/// Error occurring on calculating the shared prefix.
///
/// Occurs if the grouping in bits for the computation is invalid.
#[derive(Debug, Eq, PartialEq, Display, Error)]
pub enum GroupingError {
    #[display(
        "Invalid Grouping: has to be <= {}, but was {}",
        NodeId::BITS,
        group_size
    )]
    Invalid { group_size: NonZeroU8 },
}

// /// Creates a [NodeId] from a byte array. The resulting [NodeId] has the same size as the given array.
// impl From<[u8; NodeId::SIZE]> for NodeId {
//     fn from(inner: [u8; NodeId::SIZE]) -> Self {
//         Self { bytes: inner }
//     }
// }

impl Default for NodeId {
    fn default() -> Self {
        Self::UNDEFINED
    }
}

// ============ Conversions ==================

impl AsRef<u128> for NodeId {
    fn as_ref(&self) -> &u128 {
        &self.node_id
    }
}

// impl AsRef<[u8]> for NodeId {
//     fn as_ref(&self) -> &[u8] {
//         self.node_id.to_be_bytes().as_ref()
//     }
// }

impl From<[u8; NodeId::SIZE]> for NodeId {
    fn from(bytearray: [u8; NodeId::SIZE]) -> Self {
        let mut u = [0u8; 16];
        u[2..].clone_from_slice(&bytearray);
        let v = u128::from_be_bytes(u);
        Self { node_id: v }
    }
}

/// convert to NodeId from u128
/// panics if u128 is longer than Self::BITS
impl From<u128> for NodeId {
    fn from(id: u128) -> Self {
        if (!Self::MAX_UVAL & id) == 0 {
            Self {
                node_id: id & Self::MAX_UVAL,
            }
        } else {
            panic!("u128 should only contain {} bits", Self::BITS);
        }
    }
}

impl From<NodeId> for u128 {
    fn from(id: NodeId) -> Self {
        id.node_id & Self::MAX
    }
}

// ============ Operations ============

impl BitXor for NodeId {
    type Output = NodeId;

    fn bitxor(self, rhs: Self) -> Self::Output {
        NodeId {
            node_id: self.node_id ^ rhs.node_id,
        }
    }
}

impl BitXor for &NodeId {
    type Output = NodeId;

    fn bitxor(self, rhs: Self) -> Self::Output {
        *self ^ *rhs
    }
}

impl FromStr for NodeId {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex_str = s.trim().to_lowercase();
        match u128::from_str_radix(&hex_str, 16) {
            Ok(value) => Ok(NodeId::from(value)),
            Err(e) => Err(e),
        }
    }
}

// ============ Output Formatters ============

impl LowerHex for NodeId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        <u128 as LowerHex>::fmt(&self.node_id, f)
    }
}

impl UpperHex for NodeId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        <u128 as UpperHex>::fmt(&self.node_id, f)
    }
}

impl Debug for NodeId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "NodeId({self:028x})")
    }
}

impl From<&NodeId> for Ipv6Addr {
    fn from(value: &NodeId) -> Self {
        // FIXME this should use a configurable prefix
        let addr = 0xfc00_0000_0000_0000_0000_0000_0000_0000 | (value.node_id & NodeId::MAX_UVAL);

        Ipv6Addr::from(addr)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::num::NonZeroU8;
    use std::str::FromStr;

    use crate::domain::SharedPrefix;

    use super::NodeId;

    #[test]
    fn prefix_one() {
        assert_eq!(NodeId::MAX.prefix(1), NodeId::with_msb(0x80))
    }

    #[test]
    fn from_hex_string() -> Result<(), Box<dyn Error>> {
        let raw = "0123456789ABCDEF0123456789AB";
        let id = NodeId::from_str(raw)?;
        //assert_eq!(id.size(), NodeId::SIZE);
        assert_eq!(format!("{id:028X}"), raw);
        assert_eq!(format!("{id:028x}"), raw.to_lowercase());

        // need to fill in leading zeros
        let short = "8702fcd81b5d24bace4307bf326";
        let id = NodeId::from_str(short)?;
        assert_eq!(format!("{id:028x}"), "08702fcd81b5d24bace4307bf326");

        // test debug format
        assert_eq!(format!("{id:?}"), "NodeId(08702fcd81b5d24bace4307bf326)");

        Ok(())
    }

    #[test]
    #[should_panic]
    fn invalid_hex_string() {
        // Invalid bit length
        let _u = NodeId::from_str("123456789abcdef0123456789abcd").unwrap();
    }

    // Basic construction Operations. Testing basic construction.

    #[test]
    fn zero_is_zero() {
        assert!(NodeId::ZERO.is_zero());
    }

    #[test]
    fn zero_is_not_one() {
        assert!(!NodeId::ZERO.is_one())
    }

    #[test]
    fn one_is_not_zero() {
        assert!(!NodeId::ONE.is_zero());
    }

    #[test]
    fn one_is_one() {
        assert!(NodeId::ONE.is_one());
    }

    #[test]
    fn rand_construction_smoke_test() {
        let random = NodeId::random();
        assert!(!random.is_zero());
        assert!(!random.is_one());
    }

    #[test]
    #[should_panic(expected = "u128 should only contain 112 bits")]
    fn from_u128_panics() {
        let t: u128 = 0x1234_5678_abcd_ef12_3456_7890_abcd_ef01;
        let _p = NodeId::from(t);
    }

    #[test]
    fn from_u128() {
        let t: u128 = 0x1234_5678_abcd_ef12_3456_7890_abcd_ef01;
        let s: u128 = 0x0000_5678_abcd_ef12_3456_7890_abcd_ef01;
        let n: NodeId = NodeId::from(t & NodeId::MAX_UVAL);
        assert!(u128::from(n) == s);
    }

    // Equality Tests

    #[test]
    fn node_id_comparison() {
        assert_eq!(NodeId::ONE, NodeId::ONE);
        assert_ne!(NodeId::ONE, NodeId::ZERO);
        assert_eq!(NodeId::ZERO, NodeId::ZERO);
        let t: u128 = 0x0000_1234_5678_abcd_ef12_3456_7890_abcd;
        assert_eq!(
            NodeId::from(t),
            NodeId::from_str("12345678abcdef1234567890abcd").unwrap()
        );
        let s: u128 = 0x0000_1234_5678_abcd_ef12_3456_0987_dcba;
        assert_ne!(NodeId::from(t), NodeId::from(s));
    }

    // Xor Tests

    #[test]
    fn node_id_xor_works() {
        assert_eq!(
            NodeId::from(0x0000_1011_0000_0000_0000_0000_0000_1101u128)
                ^ NodeId::from(0x0000_1011_0000_0000_0000_0000_0000_1101u128),
            NodeId::ZERO
        );
        assert_eq!(
            NodeId::from(0x0000_1011_0000_0000_0000_0000_0000_1101u128)
                ^ NodeId::from(0x0000_0100_0000_0000_0000_0000_0000_0010u128),
            NodeId::from(0x0000_1111_0000_0000_0000_0000_0000_1111u128)
        );
    }

    #[test]
    fn prefix_bits_smoke_test() {
        let zero = NodeId::ZERO;
        let one = NodeId::with_msb(1);

        assert_eq!(zero.shared_prefix_bits(&one).bit_len(), 7);

        let zero = NodeId::ZERO;
        let one = NodeId::ONE;

        assert_eq!(zero.shared_prefix_bits(&one).bit_len(), NodeId::BITS - 1);

        let one = NodeId::ONE;
        let valid = NodeId::from(0x0000_0001_80FF_0000_0000_0000_0000u128);

        assert_eq!(one.shared_prefix_bits(&valid).bit_len(), 31);

        let one = NodeId::ONE;
        let valid = NodeId::from(0x0000_0000_80FF_0000_0000_0000_0000u128);

        assert_eq!(one.shared_prefix_bits(&valid).bit_len(), 32);
    }

    #[test]
    fn prefix_len_self() {
        assert_eq!(
            NodeId::ZERO.shared_prefix_len(&NodeId::ZERO, NonZeroU8::MIN),
            Ok(SharedPrefix {
                xor: NodeId::ZERO,
                length: NodeId::BITS,
            })
        );
    }

    #[test]
    fn prefix_length_smoke_test() {
        let zero = NodeId::ZERO;
        let one = NodeId::ONE;

        assert_eq!(
            zero.shared_prefix_len(&one, NonZeroU8::new(1).unwrap())
                .map(SharedPrefix::into_bit_len),
            Ok(NodeId::BITS - 1)
        );

        let first = NodeId::from(0x1234_5678_9a3f_0000_0000_0000_1101u128);
        let secnd = NodeId::from(0x1234_5678_9a4f_0000_0000_0000_1101u128);
        assert_eq!(
            first
                .shared_prefix_len(&secnd, NonZeroU8::new(1).unwrap())
                .map(SharedPrefix::into_bit_len),
            Ok(41)
        );

        assert_eq!(
            zero.shared_prefix_len(&one, NonZeroU8::new(2).unwrap())
                .map(SharedPrefix::into_bit_len),
            Ok((NodeId::BITS / 2) - 1)
        );
        assert_eq!(
            zero.shared_prefix_len(&one, NonZeroU8::new(4).unwrap())
                .map(SharedPrefix::into_bit_len),
            Ok((NodeId::BITS / 4) - 1)
        );

        let valid = NodeId::from(0b10000000_11111111_00000000);

        assert_eq!(
            one.shared_prefix_len(&valid, NonZeroU8::new(8).unwrap())
                .map(SharedPrefix::into_bit_len),
            Ok(11)
        );
    }

    #[test]
    fn test_single_bit() {
        let value = NodeId::from(0b01101110u128);
        assert_eq!(value.bits(0, NonZeroU8::new(1).unwrap()), Ok(0));
        assert_eq!(value.bits(1, NonZeroU8::new(1).unwrap()), Ok(1));
        assert_eq!(value.bits(2, NonZeroU8::new(1).unwrap()), Ok(1));
        assert_eq!(value.bits(3, NonZeroU8::new(1).unwrap()), Ok(1));
        assert_eq!(value.bits(4, NonZeroU8::new(1).unwrap()), Ok(0));
        assert_eq!(value.bits(5, NonZeroU8::new(1).unwrap()), Ok(1));
        assert_eq!(value.bits(6, NonZeroU8::new(1).unwrap()), Ok(1));
        assert_eq!(value.bits(7, NonZeroU8::new(1).unwrap()), Ok(0));
    }

    #[test]
    fn test_single_bits() {
        let value = NodeId::from(0b01101110u128);
        assert_eq!(value.bits(0, NonZeroU8::MIN), Ok(0));
        assert_eq!(value.bits(1, NonZeroU8::MIN), Ok(1));
        assert_eq!(value.bits(2, NonZeroU8::MIN), Ok(1));
        assert_eq!(value.bits(3, NonZeroU8::MIN), Ok(1));
        assert_eq!(value.bits(4, NonZeroU8::MIN), Ok(0));
        assert_eq!(value.bits(5, NonZeroU8::MIN), Ok(1));
        assert_eq!(value.bits(6, NonZeroU8::MIN), Ok(1));
        assert_eq!(value.bits(7, NonZeroU8::MIN), Ok(0));
    }

    #[test]
    fn test_multiple_bits_in_same_byte() {
        let value = NodeId::from(0b01101110);
        assert_eq!(value.bits(0, NonZeroU8::new(8).unwrap()), Ok(0b01101110));
        let high = NodeId::with_msb(0b11000000);
        assert_eq!(high.bits(110, NonZeroU8::new(2).unwrap()), Ok(3));
    }

    #[test]
    fn test_multiple_bits_through_multiple_bytes() {
        let value = NodeId::from(0b01101110_10110100);
        assert_eq!(value.bits(4, NonZeroU8::new(8).unwrap()), Ok(0b11101011));
    }

    #[test]
    fn test_out_of_bounds() {
        let value = NodeId::from(0b01101110_10110100);
        assert!(
            value
                .bits(NodeId::BITS + 10, NonZeroU8::new(8).unwrap())
                .is_err()
        );
        assert!(
            value
                .bits(NodeId::BITS - 10, NonZeroU8::new(18).unwrap())
                .is_err()
        );
    }

    #[test]
    fn test_multiple_bits_starting_in_higher_byte() {
        let value = NodeId::from(0b01101110_10110100_10110100_10110100u128);
        assert_eq!(value.bits(17, NonZeroU8::new(7).unwrap()), Ok(0b1011010));
    }

    #[test]
    fn test_get_bit() {
        let zero = NodeId::from(0b01101110u128);
        assert_eq!(zero.bit(0), Ok(0));
        assert_eq!(zero.bit(1), Ok(1));
        assert_eq!(zero.bit(2), Ok(1));
        assert_eq!(zero.bit(3), Ok(1));
        assert_eq!(zero.bit(4), Ok(0));
        assert_eq!(zero.bit(5), Ok(1));
        assert_eq!(zero.bit(6), Ok(1));
        assert_eq!(zero.bit(7), Ok(0));
    }
}
