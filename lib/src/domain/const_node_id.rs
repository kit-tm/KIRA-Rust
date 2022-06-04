use std::fmt::Debug;
use std::fmt::{Display, Formatter, LowerHex, UpperHex};
use std::ops::BitXor;
use std::str::FromStr;

use hex::FromHexError;

use crate::domain::{Id, DEFAULT_ID_SIZE};

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
pub struct ConstNodeId<const SIZE: usize = DEFAULT_ID_SIZE> {
    // Sorted from MSB to LSB (Big Endian representation)
    inner: [u8; SIZE],
}

// ============ Initializers ============

/// Constant initializers and functions related to them.
impl<const SIZE: usize> ConstNodeId<SIZE> {
    /// Creating a [ConstNodeId] with the numerical value of 0.
    pub const fn zero() -> Self {
        Self { inner: [0u8; SIZE] }
    }

    /// Creating a [ConstNodeId] with the numerical value of 1.
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

    /// Creates a random [ConstNodeId].
    /// The generated [ConstNodeId] is guaranteed to not be equal to [ConstNodeId::one] or [ConstNodeId::zero].
    pub fn random() -> Self {
        let mut inner = [0u8; SIZE];
        let mut rng = rand::thread_rng();
        while inner == Self::one().inner || inner == Self::zero().inner {
            rand::Rng::fill(&mut rng, &mut inner[..SIZE]);
        }
        Self { inner }
    }

    /// Checks if the [ConstNodeId] is equal to the numerical value of 0;
    pub fn is_zero(&self) -> bool {
        self.inner == [0u8; SIZE]
    }

    /// Checks if the [ConstNodeId] is equal to the numerical value of 1;
    pub fn is_one(&self) -> bool {
        if SIZE == 0 {
            return false;
        }
        self.inner[..(SIZE - 1)] == [0u8; SIZE][..SIZE - 1] && self.inner[SIZE - 1] == 1u8
    }

    pub const fn size(&self) -> usize {
        SIZE
    }
}

/// Creates a [ConstNodeId] from a byte array. The resulting [ConstNodeId] has the same size as the given array.
impl<const SIZE: usize> From<[u8; SIZE]> for ConstNodeId<SIZE> {
    fn from(inner: [u8; SIZE]) -> Self {
        Self { inner }
    }
}

impl From<u128> for ConstNodeId<16> {
    fn from(val: u128) -> Self {
        Self {
            inner: val.to_be_bytes(),
        }
    }
}

// ============ Conversions ==================

impl<const SIZE: usize> AsRef<[u8]> for ConstNodeId<SIZE> {
    fn as_ref(&self) -> &[u8] {
        self.inner.as_ref()
    }
}

// ============ Operations ============

impl<'a, const SIZE: usize> BitXor for &'a ConstNodeId<SIZE> {
    type Output = ConstNodeId<SIZE>;

    fn bitxor(self, rhs: Self) -> Self::Output {
        let mut result = [0u8; SIZE];
        for (i, item) in result.iter_mut().enumerate() {
            *item = self.inner[i] ^ rhs.inner[i];
        }
        ConstNodeId { inner: result }
    }
}

impl<const SIZE: usize> BitXor for ConstNodeId<SIZE> {
    type Output = ConstNodeId<SIZE>;

    fn bitxor(self, rhs: Self) -> Self::Output {
        &self ^ &rhs
    }
}

impl<const SIZE: usize> PartialEq for ConstNodeId<SIZE> {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

// ============ ID Functionality ============

impl<const SIZE: usize> Id for ConstNodeId<SIZE> {
    /// Returns the shared prefix length in number of groups of bits.
    ///
    /// # Notes
    ///
    /// The RoutingSim implementation uses another algorithm which uses [GMP](https://gmplib.org)
    /// and works with limbs.
    /// This algorithm uses basic operations for computing the shared prefix.
    /// As this function is used every time a package arrives optimizing it may be
    /// worth the effort.
    fn shared_prefix_len(&self, other: &Self, bits_per_group: usize) -> usize {
        if self == other {
            return SIZE / bits_per_group;
        }

        let xor: Self = self ^ other;
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

        bit_index / bits_per_group
    }
}

const SHORT_OUTPUT_LENGTH: usize = 8;

impl<const SIZE: usize> FromStr for ConstNodeId<SIZE> {
    type Err = FromHexError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut inner = [0u8; SIZE];
        hex::decode_to_slice(s, &mut inner)?;
        Ok(Self { inner })
    }
}

// ============ Output Formatters ============

impl<const SIZE: usize> LowerHex for ConstNodeId<SIZE> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match (f.precision(), f.alternate()) {
            (Some(precision), _) => write!(f, "{}", &hex::encode(self)[..precision]),
            (None, true) => write!(f, "{}", &hex::encode(self)[..SHORT_OUTPUT_LENGTH]),
            (None, false) => write!(f, "{}", hex::encode(self)),
        }
    }
}

impl<const SIZE: usize> UpperHex for ConstNodeId<SIZE> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match (f.precision(), f.alternate()) {
            (Some(precision), _) => write!(f, "{}", &hex::encode_upper(self)[..precision]),
            (None, true) => write!(f, "{}", &hex::encode_upper(self)[..SHORT_OUTPUT_LENGTH]),
            (None, false) => write!(f, "{}", hex::encode_upper(self)),
        }
    }
}

impl<const SIZE: usize> Display for ConstNodeId<SIZE> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        UpperHex::fmt(self, f)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::str::FromStr;

    use crate::domain::Id;

    use super::ConstNodeId;

    #[test]
    fn from_hex_string() -> Result<(), Box<dyn Error>> {
        let raw = "0123456789ABCDEF";
        let id: ConstNodeId<8> = raw.parse()?;
        assert_eq!(id.size(), 8);
        assert_eq!(format!("{:X}", id), raw);
        assert_eq!(format!("{:x}", id), raw.to_lowercase());

        // Invalid bit length -> Has to be multiple of 8
        assert!(ConstNodeId::<2>::from_str("012").is_err());
        assert!(ConstNodeId::<3>::from_str("012").is_err());

        // Invalid byte length -> const generic and input size have to be the same
        assert!(ConstNodeId::<2>::from_str("0123").is_ok());
        assert!(ConstNodeId::<3>::from_str("0123").is_err());

        Ok(())
    }

    // Basic construction Operations. Testing basic construction.

    #[test]
    fn zero_is_zero() {
        assert!(ConstNodeId::<0>::zero().is_zero());
    }

    #[test]
    fn zero_is_not_one() {
        assert!(!ConstNodeId::<0>::zero().is_one())
    }

    #[test]
    fn one_is_not_zero() {
        assert!(!ConstNodeId::<1>::one().is_zero());
    }

    #[test]
    fn one_is_one() {
        assert!(ConstNodeId::<1>::one().is_one());
    }

    #[test]
    fn from_u128() {
        assert!(ConstNodeId::from(1u128).is_one());
        assert!(!ConstNodeId::from(0u128).is_one());
        assert!(ConstNodeId::from(0u128).is_zero());
    }

    #[cfg(feature = "rand")]
    #[test]
    fn rand_construction_smoke_test() {
        let random = ConstNodeId::<128>::random();
        assert!(!random.is_zero());
        assert!(!random.is_one());
    }

    #[test]
    fn from_array() {
        assert!(ConstNodeId::from([0u8; 5]).is_zero());
    }

    // Equality Tests

    #[test]
    fn node_id_comparison() {
        assert_eq!(ConstNodeId::<1>::one(), ConstNodeId::<1>::one());
        assert_ne!(ConstNodeId::<1>::one(), ConstNodeId::<1>::zero());
        assert_eq!(ConstNodeId::<1>::zero(), ConstNodeId::<1>::zero());
        assert_eq!(
            ConstNodeId::from([15u8, 14u8, 13u8]),
            ConstNodeId::from([15u8, 14u8, 13u8])
        );
        assert_ne!(
            ConstNodeId::from([13u8, 14u8, 15u8]),
            ConstNodeId::from([15u8, 14u8, 13u8])
        );
    }

    // Xor Tests

    #[test]
    fn node_id_xor_works() {
        assert_eq!(
            &ConstNodeId::from([1u8, 0u8]) ^ &ConstNodeId::from([0u8, 1u8]),
            ConstNodeId::from([1u8, 1u8])
        );
    }

    #[test]
    fn prefix_bits_smoke_test() {
        let zero = ConstNodeId::<1>::zero();
        let one = ConstNodeId::<1>::one();

        assert_eq!(zero.shared_prefix_bits(&one), 7);

        let zero = ConstNodeId::<16>::zero();
        let one = ConstNodeId::<16>::one();

        assert_eq!(zero.shared_prefix_bits(&one), 16 * 8 - 1);

        let one = ConstNodeId::<4>::one();
        let valid = ConstNodeId::from([0, 0b10000000, 0xFF, 0]);

        assert_eq!(one.shared_prefix_bits(&valid), 8);
    }

    #[test]
    fn prefix_length_smoke_test() {
        let zero = ConstNodeId::<1>::zero();
        let one = ConstNodeId::<1>::one();

        assert_eq!(zero.shared_prefix_len(&one, 1), 7);
        assert_eq!(zero.shared_prefix_len(&one, 2), 3);
        assert_eq!(zero.shared_prefix_len(&one, 3), 2);

        let zero = ConstNodeId::<16>::zero();
        let one = ConstNodeId::<16>::one();

        assert_eq!(zero.shared_prefix_len(&one, 1), 127);
        assert_eq!(zero.shared_prefix_len(&one, 2), 63);

        let one = ConstNodeId::<4>::one();
        let valid = ConstNodeId::from([0, 0b10000000, 0xFF, 0]);

        assert_eq!(one.shared_prefix_len(&valid, 8), 1);
    }
}
