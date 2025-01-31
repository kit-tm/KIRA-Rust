use std::cmp::Ordering;
use std::fmt::{Debug, Formatter, LowerHex, UpperHex};
use std::net::Ipv6Addr;
use std::num::NonZeroUsize;
use std::ops::BitXor;
use std::str::FromStr;

use derive_more::derive::{Display, Error};
use hex::FromHexError;

/// Fixed byte size of a [NodeId].
///
/// Moved to constants as the length of the [NodeId] is a global parameter not to be
/// altered between nodes like the bucket size (**k**).
pub const SIZE: usize = 14;
/// Fixed bit size of a [NodeId].
pub const BIT_SIZE: usize = SIZE * 8;
/// Length in Characters of the short output format for [NodeId]s.
const SHORT_OUTPUT_LENGTH: usize = 8;

/// A NodeId with default SIZE of 112 Bits (14 Byte) as default value as proposed in the design paper.
///
/// This implementation supports creating NodeIds with a given byte size.
///
/// As all NodeIds have to be of the same size for an application this implementation
/// uses const generics to specify its size instead of using [Vec] (which uses Heap
/// allocation by default).
#[derive(Clone, Copy, Eq, PartialEq, Hash, Display)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[display("{self:X}")]
pub struct NodeId {
    // Sorted from MSB to LSB (Big Endian representation)
    #[cfg_attr(feature = "serde", serde(with = "serde_big_array::BigArray"))]
    bytes: [u8; SIZE],
}

// ============ Initializers ============

/// Constant initializers and functions related to them.
impl NodeId {
    /// Creating a [NodeId] with the numerical value of 0.
    pub const fn zero() -> Self {
        Self { bytes: [0u8; SIZE] }
    }

    /// Creating a [NodeId] with the numerical value of 1.
    ///
    /// # Panics
    ///
    /// Panics if the SIZE is 0.
    pub const fn one() -> Self {
        let mut inner = [0u8; SIZE];
        inner[SIZE - 1] = 1u8;
        Self { bytes: inner }
    }

    /// Returns the maximum representable [NodeId] for the given Byte size.
    /// This is equal to all bits in a [NodeId] == 1.
    pub const fn max_value() -> Self {
        Self {
            bytes: [0b11111111; SIZE],
        }
    }

    /// Creates a random [NodeId].
    /// The generated [NodeId] is guaranteed to not be equal to [NodeId::one] or [NodeId::zero].
    pub fn random() -> Self {
        let mut inner = [0u8; SIZE];
        let mut rng = rand::thread_rng();
        while inner == Self::one().bytes || inner == Self::zero().bytes {
            rand::Rng::fill(&mut rng, &mut inner[..SIZE]);
        }
        Self { bytes: inner }
    }

    /// Creates an all zeroed-out id with the LSB set to the given value.
    pub fn with_lsb(lsb: u8) -> Self {
        let mut id = Self::zero();
        id.bytes[SIZE - 1] = lsb;
        id
    }

    /// Creates an all zeroed-out id with the LSB set to the given value.
    pub fn with_msb(msb: u8) -> Self {
        let mut id = Self::zero();
        id.bytes[0] = msb;
        id
    }

    /// Checks if the [NodeId] is equal to the numerical value of 0;
    pub fn is_zero(&self) -> bool {
        self.bytes == [0u8; SIZE]
    }

    /// Checks if the [NodeId] is equal to the numerical value of 1;
    pub fn is_one(&self) -> bool {
        if SIZE == 0 {
            return false;
        }
        self.bytes[..(SIZE - 1)] == [0u8; SIZE][..SIZE - 1] && self.bytes[SIZE - 1] == 1u8
    }

    /// Returns the byte size of the [NodeId].
    pub const fn size(&self) -> usize {
        self.bytes.len()
    }

    /// Returns the shared prefix length in number of bits
    pub fn shared_prefix_bits(&self, other: &Self) -> Result<SharedPrefix, GroupingError> {
        self.shared_prefix_len(other, 1)
    }

    /// Returns the shared prefix length in number of groups of bits.
    ///
    /// # Notes
    ///
    /// The RoutingSim implementation uses another algorithm which uses [GMP](https://gmplib.org)
    /// and works with limbs.
    /// This algorithm uses basic operations for computing the shared prefix.
    /// As this function is used every time a package arrives optimizing it may be
    /// worth the effort.
    ///
    /// # Panics
    ///
    /// When *bits_per_group* > SIZE.
    pub fn shared_prefix_len(
        &self,
        other: &Self,
        bits_per_group: usize,
    ) -> Result<SharedPrefix, GroupingError> {
        if bits_per_group > BIT_SIZE {
            return Err(GroupingError::Invalid {
                group_size: bits_per_group,
                id_size: SIZE,
            });
        }

        let xor: Self = self ^ other;

        if self == other {
            return Ok(SharedPrefix {
                xor,
                length: BIT_SIZE / bits_per_group,
            });
        }

        let mut byte_index = 0;
        let mut not_zero_byte = None;
        for i in xor.bytes {
            match i {
                0 => byte_index += 1,
                i => {
                    not_zero_byte = Some(i);
                    break;
                }
            }
        }
        let mut in_byte_index = 0;
        if let Some(mut not_zero_byte) = not_zero_byte {
            while not_zero_byte != 0 {
                in_byte_index += 1;
                not_zero_byte >>= 1;
            }
            in_byte_index = 8 - in_byte_index;
        }
        let bit_index = byte_index * 8 + in_byte_index;

        Ok(SharedPrefix {
            length: bit_index / bits_per_group,
            xor,
        })
    }

    /// Returns the bit at the index starting from LSB. Returns either 0 or 1.
    pub fn bit(&self, bit_index: usize) -> Result<u8, BitIndexOutOfBounds> {
        let byte = bit_index / 8;
        if byte > SIZE {
            return Err(BitIndexOutOfBounds);
        }
        let byte_offset = bit_index % 8;

        let byte = SIZE - 1 - byte;
        let byte_offset = 8 - byte_offset;

        let byte = self.bytes[byte];

        Ok((byte << (byte_offset - 1)) >> 7)
    }

    /// Returns a number representing the bits from an inclusive position from LSB to MSB.
    pub fn bits(
        &self,
        from_bit_index: usize,
        num_bits: NonZeroUsize,
    ) -> Result<usize, BitIndexOutOfBounds> {
        let num_bits = num_bits.get();
        if from_bit_index + num_bits > BIT_SIZE {
            return Err(BitIndexOutOfBounds);
        }

        let mut result = 0usize;

        for index in 0..num_bits {
            result <<= 1;
            result |= self.bit(from_bit_index + (num_bits - 1 - index))? as usize;
        }

        Ok(result)
    }

    /// Returns the prefix of the given length with the rest set to 0
    pub fn prefix(&self, prefix_len: usize) -> Self {
        let mut new_bytes = self.bytes;
        let full_bytes = prefix_len / 8;
        let partial_bits = prefix_len % 8;

        if partial_bits > 0 && full_bytes < SIZE {
            let mask = 0xFFu8 << (8 - partial_bits);
            new_bytes[full_bytes] &= mask;
        }

        let start_zeroing_index = if partial_bits > 0 {
            full_bytes + 1
        } else {
            full_bytes
        };

        for byte in new_bytes.iter_mut().skip(start_zeroing_index) {
            *byte = 0;
        }

        log::trace!(target: "node_id", "prefix with length {} of {:?} is {:?}", prefix_len, self, NodeId { bytes: new_bytes });

        NodeId { bytes: new_bytes }
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
    prefix_length: usize,
}

#[derive(Debug, Copy, Clone, Display, Error)]
#[display("Prefix length is larger then bits of NodeId")]
pub struct PrefixLengthError;

impl NodeIdSubnet {
    /// Creates a new [NodeIdSubnet].
    ///
    /// This function return [PrefixLengthError] if the `prefix_length`
    /// is larger then the [bytes length](SIZE) of a [NodeId].
    pub fn try_new(
        node_id: NodeId,
        prefix_length: usize,
    ) -> Result<NodeIdSubnet, PrefixLengthError> {
        if prefix_length <= SIZE {
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
    /// The prefix length is in between zero an [SIZE].
    pub const fn prefix_length(&self) -> usize {
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
    ///
    /// If the prefix length is greater than 128 bits or zero it is clamped to 128 bits.
    pub const fn ipv6_subnet_prefix_length(&self) -> u8 {
        if self.prefix_length() == 0 || self.prefix_length() > 128 {
            128
        } else {
            16 + self.prefix_length() as u8
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
    pub(crate) length: usize,
}

impl From<SharedPrefix> for usize {
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

    pub fn bit_len(&self) -> usize {
        self.length
    }

    pub fn into_bit_len(self) -> usize {
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
    #[display("Invalid Grouping: has to be <= {}, but was {}", id_size * 8, group_size)]
    Invalid { group_size: usize, id_size: usize },
}

/// Creates a [NodeId] from a byte array. The resulting [NodeId] has the same size as the given array.
impl From<[u8; SIZE]> for NodeId {
    fn from(inner: [u8; SIZE]) -> Self {
        Self { bytes: inner }
    }
}

// ============ Conversions ==================

impl AsRef<[u8]> for NodeId {
    fn as_ref(&self) -> &[u8] {
        self.bytes.as_ref()
    }
}

impl From<NodeId> for [u8; SIZE] {
    fn from(id: NodeId) -> Self {
        id.bytes
    }
}

// ============ Operations ============

impl BitXor for NodeId {
    type Output = NodeId;

    fn bitxor(self, rhs: Self) -> Self::Output {
        if self == NodeId::zero() {
            return rhs;
        }
        if rhs == NodeId::zero() {
            return self;
        }

        let mut result = [0u8; SIZE];
        for (i, item) in result.iter_mut().enumerate() {
            *item = self.bytes[i] ^ rhs.bytes[i];
        }
        NodeId { bytes: result }
    }
}

impl BitXor for &NodeId {
    type Output = NodeId;

    fn bitxor(self, rhs: Self) -> Self::Output {
        *self ^ *rhs
    }
}

impl FromStr for NodeId {
    type Err = FromHexError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut inner = [0u8; SIZE];
        hex::decode_to_slice(s, &mut inner)?;
        Ok(Self { bytes: inner })
    }
}

impl PartialOrd for NodeId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for NodeId {
    fn cmp(&self, other: &Self) -> Ordering {
        for (own_byte, other_byte) in self.bytes.iter().zip(other.bytes.iter()) {
            match own_byte.cmp(other_byte) {
                Ordering::Greater => return Ordering::Greater,
                Ordering::Less => return Ordering::Less,
                _ => continue,
            }
        }

        Ordering::Equal
    }
}

// ============ Output Formatters ============

impl LowerHex for NodeId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match (f.precision(), f.alternate()) {
            (Some(precision), _) => write!(f, "{}", &hex::encode(self)[..precision]),
            (None, true) => write!(f, "{}", &hex::encode(self)[..SHORT_OUTPUT_LENGTH]),
            (None, false) => write!(f, "{}", hex::encode(self)),
        }
    }
}

impl UpperHex for NodeId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match (f.precision(), f.alternate()) {
            (Some(precision), _) => write!(f, "{}", &hex::encode_upper(self)[..precision]),
            (None, true) => write!(f, "{}", &hex::encode_upper(self)[..SHORT_OUTPUT_LENGTH]),
            (None, false) => write!(f, "{}", hex::encode_upper(self)),
        }
    }
}

impl Debug for NodeId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "NodeId({:X})", self)
    }
}

impl From<&NodeId> for Ipv6Addr {
    fn from(value: &NodeId) -> Self {
        let mut bytes = [0; 16];
        bytes[0] = 0xfc;
        bytes[1] = 0x00;
        bytes[2..16].copy_from_slice(&value.bytes[0..14]);
        Ipv6Addr::from(bytes)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::num::NonZeroUsize;
    use std::str::FromStr;

    use crate::domain::{node_id, SharedPrefix};

    use super::NodeId;

    #[test]
    fn prefix_one() {
        assert_eq!(NodeId::max_value().prefix(1), NodeId::with_msb(0x80))
    }

    #[test]
    fn from_hex_string() -> Result<(), Box<dyn Error>> {
        let raw = "0123456789ABCDEF0123456789AB";
        let id = NodeId::from_str(raw)?;
        assert_eq!(id.size(), node_id::SIZE);
        assert_eq!(format!("{:X}", id), raw);
        assert_eq!(format!("{:x}", id), raw.to_lowercase());

        // Invalid bit length -> Has to be multiple of 8
        assert!(NodeId::from_str("0123456789ABCDEF0123456789A").is_err());
        assert!(NodeId::from_str("0123456789ABCDEF0123456789ABC").is_err());

        Ok(())
    }

    // Basic construction Operations. Testing basic construction.

    #[test]
    fn zero_is_zero() {
        assert!(NodeId::zero().is_zero());
    }

    #[test]
    fn zero_is_not_one() {
        assert!(!NodeId::zero().is_one())
    }

    #[test]
    fn one_is_not_zero() {
        assert!(!NodeId::one().is_zero());
    }

    #[test]
    fn one_is_one() {
        assert!(NodeId::one().is_one());
    }

    #[test]
    fn rand_construction_smoke_test() {
        let random = NodeId::random();
        assert!(!random.is_zero());
        assert!(!random.is_one());
    }

    #[test]
    fn from_array() {
        assert!(NodeId::from([0u8; 14]).is_zero());
    }

    // Equality Tests

    #[test]
    fn node_id_comparison() {
        assert_eq!(NodeId::one(), NodeId::one());
        assert_ne!(NodeId::one(), NodeId::zero());
        assert_eq!(NodeId::zero(), NodeId::zero());
        assert_eq!(
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 15u8, 14u8, 13u8]),
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 15u8, 14u8, 13u8])
        );
        assert_ne!(
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 13u8, 14u8, 15u8]),
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 15u8, 14u8, 13u8])
        );
    }

    // Xor Tests

    #[test]
    fn node_id_xor_works() {
        assert_eq!(
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1u8, 0u8])
                ^ NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0u8, 1u8]),
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1u8, 1u8])
        );
    }

    #[test]
    fn prefix_bits_smoke_test() {
        let zero = NodeId::zero();
        let one = NodeId::with_msb(1);

        assert_eq!(
            zero.shared_prefix_bits(&one)
                .map(SharedPrefix::into_bit_len),
            Ok(7)
        );

        let zero = NodeId::zero();
        let one = NodeId::one();

        assert_eq!(
            zero.shared_prefix_bits(&one)
                .map(SharedPrefix::into_bit_len),
            Ok(node_id::BIT_SIZE - 1)
        );

        let one = NodeId::one();
        let valid = NodeId::from([0, 0b10000000, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);

        assert_eq!(
            one.shared_prefix_bits(&valid)
                .map(SharedPrefix::into_bit_len),
            Ok(8)
        );
    }

    #[test]
    fn prefix_len_self() {
        assert_eq!(
            NodeId::zero().shared_prefix_len(&NodeId::zero(), 1),
            Ok(SharedPrefix {
                xor: NodeId::zero(),
                length: node_id::BIT_SIZE,
            })
        );
    }

    #[test]
    fn prefix_length_smoke_test() {
        let zero = NodeId::zero();
        let one = NodeId::one();

        assert_eq!(
            zero.shared_prefix_len(&one, 1)
                .map(SharedPrefix::into_bit_len),
            Ok(node_id::BIT_SIZE - 1)
        );
        assert_eq!(
            zero.shared_prefix_len(&one, 2)
                .map(SharedPrefix::into_bit_len),
            Ok((node_id::BIT_SIZE / 2) - 1)
        );
        assert_eq!(
            zero.shared_prefix_len(&one, 4)
                .map(SharedPrefix::into_bit_len),
            Ok((node_id::BIT_SIZE / 4) - 1)
        );

        let one = NodeId::one();
        let valid = NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0b10000000, 0xFF, 0]);

        assert_eq!(
            one.shared_prefix_len(&valid, 8)
                .map(SharedPrefix::into_bit_len),
            Ok(11)
        );
    }

    #[test]
    fn test_single_bits() {
        let value = NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0b01101110]);
        assert_eq!(value.bits(0, NonZeroUsize::new(1).unwrap()), Ok(0));
        assert_eq!(value.bits(1, NonZeroUsize::new(1).unwrap()), Ok(1));
        assert_eq!(value.bits(2, NonZeroUsize::new(1).unwrap()), Ok(1));
        assert_eq!(value.bits(3, NonZeroUsize::new(1).unwrap()), Ok(1));
        assert_eq!(value.bits(4, NonZeroUsize::new(1).unwrap()), Ok(0));
        assert_eq!(value.bits(5, NonZeroUsize::new(1).unwrap()), Ok(1));
        assert_eq!(value.bits(6, NonZeroUsize::new(1).unwrap()), Ok(1));
        assert_eq!(value.bits(7, NonZeroUsize::new(1).unwrap()), Ok(0));
    }

    #[test]
    fn test_multiple_bits_in_same_byte() {
        let value = NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0b01101110]);
        assert_eq!(value.bits(0, NonZeroUsize::new(8).unwrap()), Ok(0b01101110));
    }

    #[test]
    fn test_multiple_bits_through_multiply_bytes() {
        let value = NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0b01101110, 0b10110100]);
        assert_eq!(value.bits(4, NonZeroUsize::new(8).unwrap()), Ok(0b11101011));
    }

    #[test]
    fn test_out_of_bounds() {
        let value = NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0b01101110, 0b10110100]);
        assert!(value
            .bits(node_id::BIT_SIZE + 10, NonZeroUsize::new(8).unwrap())
            .is_err());
    }

    #[test]
    fn test_multiple_bits_starting_in_higher_byte() {
        let value = NodeId::from([
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0b01101110, 0b10110100, 0b10110100, 0b10110100,
        ]);
        assert_eq!(value.bits(17, NonZeroUsize::new(7).unwrap()), Ok(0b1011010));
    }

    #[test]
    fn test_get_bit() {
        let zero = NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0b01101110]);
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
