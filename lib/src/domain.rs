use std::ops::BitXor;

use rand::Rng;

pub const DEFAULT_SIZE: usize = 14;

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
    // Sorted from MSB to LSB (Big Endian representation)
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

    /// Creates a random [NodeID].
    /// The generated [NodeID] is guaranteed to not be equal to [NodeID::one] or [NodeID::zero].
    #[doc_cfg::doc_cfg(feature = "rand")]
    #[cfg(feature = "rand")]
    pub fn random() -> Self {
        let mut inner = [0u8; SIZE];
        let mut rng = rand::thread_rng();
        while inner == Self::one().inner || inner == Self::zero().inner {
            rng.fill(&mut inner[..SIZE]);
        }
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

    pub const fn size(&self) -> usize {
        SIZE
    }

    /// Implements calculating the shared prefix length by bits.
    pub fn shared_prefix_bits(&self, other: &Self) -> usize {
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
    pub fn shared_prefix_len(&self, other: &Self, bits_per_group: usize) -> usize {
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

        return bit_index / bits_per_group;
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

// ============ Operations ============

impl<'a, const SIZE: usize> BitXor for &'a NodeID<SIZE> {
    type Output = NodeID<SIZE>;

    fn bitxor(self, rhs: Self) -> Self::Output {
        let mut result = [0u8; SIZE];
        for (i, item) in result.iter_mut().enumerate() {
            *item = self.inner[i] ^ rhs.inner[i];
        }
        NodeID { inner: result }
    }
}

impl<const SIZE: usize> BitXor for NodeID<SIZE> {
    type Output = NodeID<SIZE>;

    fn bitxor(self, rhs: Self) -> Self::Output {
        &self ^ &rhs
    }
}

impl<const SIZE: usize> PartialEq for NodeID<SIZE> {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

#[cfg(feature = "hex")]
mod hex {
    use std::fmt::{Display, Formatter, LowerHex, UpperHex};
    use std::str::FromStr;

    use hex::FromHexError;

    use crate::domain::NodeID;

    const SHORT_OUTPUT_LENGTH: usize = 8;

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

    #[cfg(test)]
    mod tests {
        use std::error::Error;
        use std::str::FromStr;

        use crate::domain::NodeID;

        #[test]
        fn from_hex_string() -> Result<(), Box<dyn Error>> {
            let raw = "0123456789ABCDEF";
            let id: NodeID<8> = raw.parse()?;
            assert_eq!(id.size(), 8);
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
    }
}

#[cfg(test)]
mod tests {
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

    #[cfg(feature = "rand")]
    #[test]
    fn rand_construction_smoke_test() {
        let random = NodeID::<128>::random();
        assert!(!random.is_zero());
        assert!(!random.is_one());
    }

    #[test]
    fn from_array() {
        assert!(NodeID::from([0u8; 5]).is_zero());
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
            &NodeID::from([1u8, 0u8]) ^ &NodeID::from([0u8, 1u8]),
            NodeID::from([1u8, 1u8])
        );
    }

    #[test]
    fn prefix_bits_smoke_test() {
        let zero = NodeID::<1>::zero();
        let one = NodeID::<1>::one();

        assert_eq!(zero.shared_prefix_bits(&one), 7);

        let zero = NodeID::<16>::zero();
        let one = NodeID::<16>::one();

        assert_eq!(zero.shared_prefix_bits(&one), 16 * 8 - 1);

        let one = NodeID::<4>::one();
        let valid = NodeID::from([0, 0b10000000, 0xFF, 0]);

        assert_eq!(one.shared_prefix_bits(&valid), 8);
    }

    #[test]
    fn prefix_length_smoke_test() {
        let zero = NodeID::<1>::zero();
        let one = NodeID::<1>::one();

        assert_eq!(zero.shared_prefix_len(&one, 1), 7);
        assert_eq!(zero.shared_prefix_len(&one, 2), 3);
        assert_eq!(zero.shared_prefix_len(&one, 3), 2);

        let zero = NodeID::<16>::zero();
        let one = NodeID::<16>::one();

        assert_eq!(zero.shared_prefix_len(&one, 1), 127);
        assert_eq!(zero.shared_prefix_len(&one, 2), 63);

        let one = NodeID::<4>::one();
        let valid = NodeID::from([0, 0b10000000, 0xFF, 0]);

        assert_eq!(one.shared_prefix_len(&valid, 8), 1);
    }
}
