use std::fmt::{Display, Formatter, LowerHex, UpperHex};
use std::ops::BitXor;
use std::str::FromStr;

use hex::FromHexError;

pub const DEFAULT_SIZE: usize = 14;
const SHORT_OUTPUT_LENGTH: usize = 8;

/// A NodeID with default SIZE of 112 Bits (14 Byte) as default value as proposed in the design paper.
/// This implementation supports and NodeID with a given byte size.
///
/// As all NodeIDs have to be of the same size for an application this implementation
/// uses const generics to specify its size.
/// As an alternative [Vec] could be used. But [Vec] uses heap allocation by default.
///
/// TODO:
///     - Evaluate performance gains by using const generics?
///     - What should be the "default" value for a NodeID?
#[derive(Debug, Clone, Eq)]
pub struct NodeID<const SIZE: usize = DEFAULT_SIZE> {
    inner: [u8; SIZE],
}

// ============ Initializers ============

/// Constant initializers and functions related to them.
impl<const SIZE: usize> NodeID<SIZE> {
    /// Creating a [NodeID] with the numerical value of 0.
    pub const fn zero() -> Self {
        Self { inner: [0u8; SIZE] }
    }

    /// Creating a [NodeID] with the numerical value of 1.
    ///
    /// # Panics
    ///
    /// Panics if the SIZE is 0.
    pub const fn one() -> Self {
        if SIZE == 0 {
            panic!("NodeID of size 0 can't represent a value with numerical value 1");
        }

        let mut inner = [0u8; SIZE];
        inner[SIZE - 1] = 1u8;
        Self { inner }
    }

    /// Checks if the [NodeID] is equal to the numerical value of 0;
    pub fn is_zero(&self) -> bool {
        self.inner == [0u8; SIZE]
    }

    /// Checks if the [NodeID] is equal to the numerical value of 1;
    pub fn is_one(&self) -> bool {
        if SIZE == 0 {
            return false;
        }
        self.inner[..(SIZE - 1)] == [0u8; SIZE][..SIZE - 1] && self.inner[SIZE - 1] == 1u8
    }

    pub const fn len(&self) -> usize {
        SIZE
    }
}

/// Creates a [NodeID] from a byte array. The resulting [NodeID] has the same size as the given array.
impl<const SIZE: usize> From<[u8; SIZE]> for NodeID<SIZE> {
    fn from(inner: [u8; SIZE]) -> Self {
        Self { inner }
    }
}

impl From<u128> for NodeID<16> {
    fn from(val: u128) -> Self {
        Self {
            inner: val.to_be_bytes(),
        }
    }
}

// ============ Conversions ==================

impl<const SIZE: usize> AsRef<[u8]> for NodeID<SIZE> {
    fn as_ref(&self) -> &[u8] {
        self.inner.as_ref()
    }
}

impl<const SIZE: usize> FromStr for NodeID<SIZE> {
    type Err = FromHexError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut inner = [0u8; SIZE];
        hex::decode_to_slice(s, &mut inner)?;
        Ok(Self { inner })
    }
}

// ============ Output Formatters ============

impl<const SIZE: usize> LowerHex for NodeID<SIZE> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match (f.precision(), f.alternate()) {
            (Some(precision), _) => write!(f, "{}", &hex::encode(self)[..precision]),
            (None, true) => write!(f, "{}", &hex::encode(self)[..SHORT_OUTPUT_LENGTH]),
            (None, false) => write!(f, "{}", hex::encode(self)),
        }
    }
}

impl<const SIZE: usize> UpperHex for NodeID<SIZE> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match (f.precision(), f.alternate()) {
            (Some(precision), _) => write!(f, "{}", &hex::encode_upper(self)[..precision]),
            (None, true) => write!(f, "{}", &hex::encode_upper(self)[..SHORT_OUTPUT_LENGTH]),
            (None, false) => write!(f, "{}", hex::encode_upper(self)),
        }
    }
}

impl<const SIZE: usize> Display for NodeID<SIZE> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        UpperHex::fmt(self, f)
    }
}

// ============ Operations ============

impl<const SIZE: usize> BitXor for NodeID<SIZE> {
    type Output = Self;

    fn bitxor(self, rhs: Self) -> Self::Output {
        let mut result = [0u8; SIZE];
        for i in 0..SIZE {
            result[i] = self.inner[i] ^ rhs.inner[i];
        }
        Self { inner: result }
    }
}

impl<const SIZE: usize> PartialEq for NodeID<SIZE> {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::str::FromStr;

    use crate::domain::NodeID;

    // Basic construction Operations. Testing basic construction.

    #[test]
    fn zero_is_zero() {
        assert!(NodeID::<0>::zero().is_zero());
    }

    #[test]
    fn zero_is_not_one() {
        assert!(!NodeID::<0>::zero().is_one())
    }

    #[test]
    fn one_is_not_zero() {
        assert!(!NodeID::<1>::one().is_zero());
    }

    #[test]
    fn one_is_one() {
        assert!(NodeID::<1>::one().is_one());
    }

    #[test]
    fn from_u128() {
        assert!(NodeID::from(1u128).is_one());
        assert!(!NodeID::from(0u128).is_one());
        assert!(NodeID::from(0u128).is_zero());
    }

    #[test]
    fn from_array() {
        assert!(NodeID::from([0u8; 5]).is_zero());
    }

    // Formatting tests

    #[test]
    fn from_hex_string() -> Result<(), Box<dyn Error>> {
        let raw = "0123456789ABCDEF";
        let id: NodeID<8> = raw.parse()?;
        assert_eq!(id.len(), 8);
        assert_eq!(format!("{:X}", id), raw);
        assert_eq!(format!("{:x}", id), raw.to_lowercase());

        // Invalid bit length -> Has to be multiple of 8
        assert!(NodeID::<2>::from_str("012").is_err());
        assert!(NodeID::<3>::from_str("012").is_err());

        // Invalid byte length -> const generic and input size have to be the same
        assert!(NodeID::<2>::from_str("0123").is_ok());
        assert!(NodeID::<3>::from_str("0123").is_err());

        Ok(())
    }

    // Equality Tests

    #[test]
    fn node_id_comparison() {
        assert_eq!(NodeID::<1>::one(), NodeID::<1>::one());
        assert_ne!(NodeID::<1>::one(), NodeID::<1>::zero());
        assert_eq!(NodeID::<1>::zero(), NodeID::<1>::zero());
        assert_eq!(
            NodeID::from([15u8, 14u8, 13u8]),
            NodeID::from([15u8, 14u8, 13u8])
        );
        assert_ne!(
            NodeID::from([13u8, 14u8, 15u8]),
            NodeID::from([15u8, 14u8, 13u8])
        );
    }

    // Xor Tests

    #[test]
    fn node_id_xor_works() {
        assert_eq!(
            NodeID::from([1u8, 0u8]) ^ NodeID::from([0u8, 1u8]),
            NodeID::from([1u8, 1u8])
        );
    }
}
