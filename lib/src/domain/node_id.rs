use std::error::Error;
use std::fmt::Debug;
use std::fmt::{Display, Formatter, LowerHex, UpperHex};
use std::ops::{BitXor, Bound, RangeBounds};
use std::str::FromStr;

use hex::FromHexError;

pub const DEFAULT_ID_SIZE: usize = 14;

/// A NodeId with default SIZE of 112 Bits (14 Byte) as default value as proposed in the design paper.
///
/// This implementation supports creating NodeIds with a given byte size.
///
/// As all NodeIds have to be of the same size for an application this implementation
/// uses const generics to specify its size instead of using [Vec] (which uses Heap
/// allocation by default).
///
/// TODO:
///     - Evaluate performance gains by using const generics?
///     - What should be the "default" value for a NodeId?
#[derive(Debug, Clone, Eq)]
pub struct NodeId<const SIZE: usize = DEFAULT_ID_SIZE> {
    // Sorted from MSB to LSB (Big Endian representation)
    inner: [u8; SIZE],
}

// ============ Initializers ============

/// Constant initializers and functions related to them.
impl<const SIZE: usize> NodeId<SIZE> {
    /// Creating a [NodeId] with the numerical value of 0.
    pub const fn zero() -> Self {
        Self { inner: [0u8; SIZE] }
    }

    /// Creating a [NodeId] with the numerical value of 1.
    ///
    /// # Panics
    ///
    /// Panics if the SIZE is 0.
    pub const fn one() -> Self {
        if SIZE == 0 {
            panic!("NodeId of size 0 can't represent a value with numerical value 1");
        }

        let mut inner = [0u8; SIZE];
        inner[SIZE - 1] = 1u8;
        Self { inner }
    }

    /// Creates a random [NodeId].
    /// The generated [NodeId] is guaranteed to not be equal to [NodeId::one] or [NodeId::zero].
    pub fn random() -> Self {
        let mut inner = [0u8; SIZE];
        let mut rng = rand::thread_rng();
        while inner == Self::one().inner || inner == Self::zero().inner {
            rand::Rng::fill(&mut rng, &mut inner[..SIZE]);
        }
        Self { inner }
    }

    /// Checks if the [NodeId] is equal to the numerical value of 0;
    pub fn is_zero(&self) -> bool {
        self.inner == [0u8; SIZE]
    }

    /// Checks if the [NodeId] is equal to the numerical value of 1;
    pub fn is_one(&self) -> bool {
        if SIZE == 0 {
            return false;
        }
        self.inner[..(SIZE - 1)] == [0u8; SIZE][..SIZE - 1] && self.inner[SIZE - 1] == 1u8
    }

    pub const fn size(&self) -> usize {
        SIZE
    }

    /// Returns the shared prefix length in number of bits
    pub fn shared_prefix_bits(&self, other: &Self) -> Result<SharedPrefix<SIZE>, GroupingError> {
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
    ) -> Result<SharedPrefix<SIZE>, GroupingError> {
        if bits_per_group > SIZE {
            return Err(GroupingError::Invalid {
                group_size: bits_per_group,
                id_size: SIZE,
            });
        }

        let xor: Self = self ^ other;

        if self == other {
            return Ok(SharedPrefix {
                xor,
                value: SIZE / bits_per_group,
            });
        }

        let mut byte_index = 0;
        let mut not_zero_byte = None;
        for i in xor.inner {
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
            value: bit_index / bits_per_group,
            xor,
        })
    }

    /// Returns a number representing the bits in the given range from least significant
    /// to most significant bit.
    pub fn bits<T: RangeBounds<usize>>(&self, range: T) -> Result<usize, InvalidBitRange> {
        match (range.start_bound(), range.end_bound()) {
            (Bound::Unbounded, Bound::Unbounded) => Err(InvalidBitRange),
            (Bound::Unbounded, Bound::Excluded(val)) => {
                if val - 1 > 8 {
                    Err(InvalidBitRange)
                } else {
                    Ok(())
                }
            }
            (Bound::Unbounded, Bound::Included(val)) => {
                if *val > 8 {
                    Err(InvalidBitRange)
                } else {
                    Ok(())
                }
            }
            (Bound::Included(start), Bound::Excluded(end)) => {
                if end - start > 8 {
                    Err(InvalidBitRange)
                } else {
                    Ok(())
                }
            }
            (start, end) => panic!("Invalid Range: {:?} .. {:?}", start, end),
        }?;

        // Indexing requires inverting the index as id is sorted from MSB to LSB
        // and range is starting from LSB.
        let (byte_start_pos, byte_start_offset) = match range.start_bound() {
            Bound::Unbounded => (SIZE, 8),
            Bound::Excluded(val) => (SIZE - (val - 1) / 8, 8 - (val - 1) % 8),
            Bound::Included(val) => (SIZE - val / 8, 8 - val % 8),
        };

        let (byte_end_pos, byte_end_offset) = match range.end_bound() {
            Bound::Unbounded => (0, 0),
            Bound::Excluded(val) => (SIZE - (val - 1) / 8, (val - 1) % 8),
            Bound::Included(val) => (SIZE - val / 8, val % 8),
        };

        let mut result = 0usize;
        // Copy MSB
        result |=
            ((&self.inner[byte_end_pos] << (8 - byte_end_offset)) >> byte_end_offset) as usize;
        // Copy all bytes not affected by offsets
        for i in byte_end_pos..byte_start_pos {
            let byte = &self.inner[i];
            result <<= 8;
            result |= *byte as usize;
        }
        // Copy LSB
        result <<= byte_start_offset;
        result |= (&self.inner[byte_start_pos] >> byte_start_offset) as usize;
        Ok(result)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct InvalidBitRange;

impl Display for InvalidBitRange {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Invalid Bit Range: Has to be at max 64 bits long and start < end"
        )
    }
}

impl Error for InvalidBitRange {}

#[derive(Debug, Eq, PartialEq)]
pub struct SharedPrefix<const SIZE: usize> {
    pub xor: NodeId<SIZE>,
    pub value: usize,
}

#[derive(Debug, Eq, PartialEq)]
pub enum GroupingError {
    Invalid { group_size: usize, id_size: usize },
}

impl Display for GroupingError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid {
                group_size,
                id_size,
            } => write!(
                f,
                "Invalid Grouping: Has to be <= {}, but was {}",
                id_size * 8,
                group_size
            ),
        }
    }
}

impl Error for GroupingError {}

/// Creates a [NodeId] from a byte array. The resulting [NodeId] has the same size as the given array.
impl<const SIZE: usize> From<[u8; SIZE]> for NodeId<SIZE> {
    fn from(inner: [u8; SIZE]) -> Self {
        Self { inner }
    }
}

impl From<u128> for NodeId<16> {
    fn from(val: u128) -> Self {
        Self {
            inner: val.to_be_bytes(),
        }
    }
}

// ============ Conversions ==================

impl<const SIZE: usize> AsRef<[u8]> for NodeId<SIZE> {
    fn as_ref(&self) -> &[u8] {
        self.inner.as_ref()
    }
}

// ============ Operations ============

impl<'a, const SIZE: usize> BitXor for &'a NodeId<SIZE> {
    type Output = NodeId<SIZE>;

    fn bitxor(self, rhs: Self) -> Self::Output {
        let mut result = [0u8; SIZE];
        for (i, item) in result.iter_mut().enumerate() {
            *item = self.inner[i] ^ rhs.inner[i];
        }
        NodeId { inner: result }
    }
}

impl<const SIZE: usize> BitXor for NodeId<SIZE> {
    type Output = NodeId<SIZE>;

    fn bitxor(self, rhs: Self) -> Self::Output {
        &self ^ &rhs
    }
}

impl<const SIZE: usize> PartialEq for NodeId<SIZE> {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

const SHORT_OUTPUT_LENGTH: usize = 8;

impl<const SIZE: usize> FromStr for NodeId<SIZE> {
    type Err = FromHexError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut inner = [0u8; SIZE];
        hex::decode_to_slice(s, &mut inner)?;
        Ok(Self { inner })
    }
}

// ============ Output Formatters ============

impl<const SIZE: usize> LowerHex for NodeId<SIZE> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match (f.precision(), f.alternate()) {
            (Some(precision), _) => write!(f, "{}", &hex::encode(self)[..precision]),
            (None, true) => write!(f, "{}", &hex::encode(self)[..SHORT_OUTPUT_LENGTH]),
            (None, false) => write!(f, "{}", hex::encode(self)),
        }
    }
}

impl<const SIZE: usize> UpperHex for NodeId<SIZE> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match (f.precision(), f.alternate()) {
            (Some(precision), _) => write!(f, "{}", &hex::encode_upper(self)[..precision]),
            (None, true) => write!(f, "{}", &hex::encode_upper(self)[..SHORT_OUTPUT_LENGTH]),
            (None, false) => write!(f, "{}", hex::encode_upper(self)),
        }
    }
}

impl<const SIZE: usize> Display for NodeId<SIZE> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        UpperHex::fmt(self, f)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::str::FromStr;

    use crate::domain::SharedPrefix;

    use super::NodeId;

    #[test]
    fn from_hex_string() -> Result<(), Box<dyn Error>> {
        let raw = "0123456789ABCDEF";
        let id: NodeId<8> = raw.parse()?;
        assert_eq!(id.size(), 8);
        assert_eq!(format!("{:X}", id), raw);
        assert_eq!(format!("{:x}", id), raw.to_lowercase());

        // Invalid bit length -> Has to be multiple of 8
        assert!(NodeId::<2>::from_str("012").is_err());
        assert!(NodeId::<3>::from_str("012").is_err());

        // Invalid byte length -> const generic and input size have to be the same
        assert!(NodeId::<2>::from_str("0123").is_ok());
        assert!(NodeId::<3>::from_str("0123").is_err());

        Ok(())
    }

    // Basic construction Operations. Testing basic construction.

    #[test]
    fn zero_is_zero() {
        assert!(NodeId::<0>::zero().is_zero());
    }

    #[test]
    fn zero_is_not_one() {
        assert!(!NodeId::<0>::zero().is_one())
    }

    #[test]
    fn one_is_not_zero() {
        assert!(!NodeId::<1>::one().is_zero());
    }

    #[test]
    fn one_is_one() {
        assert!(NodeId::<1>::one().is_one());
    }

    #[test]
    fn from_u128() {
        assert!(NodeId::from(1u128).is_one());
        assert!(!NodeId::from(0u128).is_one());
        assert!(NodeId::from(0u128).is_zero());
    }

    #[cfg(feature = "rand")]
    #[test]
    fn rand_construction_smoke_test() {
        let random = NodeId::<128>::random();
        assert!(!random.is_zero());
        assert!(!random.is_one());
    }

    #[test]
    fn from_array() {
        assert!(NodeId::from([0u8; 5]).is_zero());
    }

    // Equality Tests

    #[test]
    fn node_id_comparison() {
        assert_eq!(NodeId::<1>::one(), NodeId::<1>::one());
        assert_ne!(NodeId::<1>::one(), NodeId::<1>::zero());
        assert_eq!(NodeId::<1>::zero(), NodeId::<1>::zero());
        assert_eq!(
            NodeId::from([15u8, 14u8, 13u8]),
            NodeId::from([15u8, 14u8, 13u8])
        );
        assert_ne!(
            NodeId::from([13u8, 14u8, 15u8]),
            NodeId::from([15u8, 14u8, 13u8])
        );
    }

    // Xor Tests

    #[test]
    fn node_id_xor_works() {
        assert_eq!(
            &NodeId::from([1u8, 0u8]) ^ &NodeId::from([0u8, 1u8]),
            NodeId::from([1u8, 1u8])
        );
    }

    #[test]
    fn prefix_bits_smoke_test() {
        let zero = NodeId::<1>::zero();
        let one = NodeId::<1>::one();

        assert_eq!(
            zero.shared_prefix_bits(&one),
            Ok(SharedPrefix {
                xor: zero ^ one,
                value: 7,
            })
        );

        let zero = NodeId::<16>::zero();
        let one = NodeId::<16>::one();

        assert_eq!(
            zero.shared_prefix_bits(&one),
            Ok(SharedPrefix {
                value: 16 * 8 - 1,
                xor: zero ^ one,
            })
        );

        let one = NodeId::<4>::one();
        let valid = NodeId::from([0, 0b10000000, 0xFF, 0]);

        assert_eq!(one.shared_prefix_bits(&valid), 8);
    }

    #[test]
    fn prefix_length_smoke_test() {
        let zero = NodeId::<1>::zero();
        let one = NodeId::<1>::one();

        assert_eq!(zero.shared_prefix_len(&one, 1), 7);
        assert_eq!(zero.shared_prefix_len(&one, 2), 3);
        assert_eq!(zero.shared_prefix_len(&one, 3), 2);

        let zero = NodeId::<16>::zero();
        let one = NodeId::<16>::one();

        assert_eq!(zero.shared_prefix_len(&one, 1), 127);
        assert_eq!(zero.shared_prefix_len(&one, 2), 63);

        let one = NodeId::<4>::one();
        let valid = NodeId::from([0, 0b10000000, 0xFF, 0]);

        assert_eq!(one.shared_prefix_len(&valid, 8), 1);
    }

    #[test]
    fn test_bit_range() {
        let zero = NodeId::<1>::zero();
        let bits = zero.bits((..));
        assert!(bits.is_ok());
        assert_eq!(bits.unwrap(), 0);
    }
}
