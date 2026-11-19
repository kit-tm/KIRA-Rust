use std::{
    num::NonZeroU32,
    ops::{
        Add,
        AddAssign,
    },
};

use derive_more::{
    Display,
    Error,
    From,
};

const INVALID_SSN: u32 = u32::MIN;
/// Reserved on the wire, decoded as [StateSeqNr::Invalid].
const RESERVED_SSN: u32 = u32::MAX;

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Display, From)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(into = "u32", from = "u32"))]
/// Number that is increased on underlay neighborhood changes of a node.
///
/// It wraps around to the smallest value after the largest one. Like after a restart of the
/// node, other nodes then see a lower number and ask the node for an authoritative answer.
pub enum StateSeqNr {
    /// Not a valid [StateSeqNr].
    Invalid,

    #[from]
    Value(SafeStateSeqNr),
}

/// Newtype that is guaranteed to be a valid state sequence number.
///
/// This excludes `0` and the reserved sequence number `0xffffffff`.
#[derive(Debug, Display, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(into = "u32", try_from = "u32"))]
pub struct SafeStateSeqNr(NonZeroU32); // NonZeroU32 for Niche Optimizations

impl SafeStateSeqNr {
    pub const MIN: Self = SafeStateSeqNr(const { NonZeroU32::MIN });
}

#[derive(Debug, Display, Error)]
#[display("Value {_0} is not a valid state sequence number.")]
pub struct InvalidStateSeqNrError(#[error(ignore)] u32);

impl From<u32> for StateSeqNr {
    fn from(value: u32) -> Self {
        match value {
            INVALID_SSN | RESERVED_SSN => Self::Invalid,
            ssn => Self::Value(ssn.try_into().expect("invalid cases should be covered")),
        }
    }
}

impl TryFrom<u32> for SafeStateSeqNr {
    type Error = InvalidStateSeqNrError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            INVALID_SSN => Err(InvalidStateSeqNrError(INVALID_SSN)),
            RESERVED_SSN => Err(InvalidStateSeqNrError(RESERVED_SSN)),
            ssn => Ok(Self(
                ssn.try_into()
                    .expect("zero should be covered by INVALID_SSN"),
            )),
        }
    }
}

impl From<StateSeqNr> for u32 {
    fn from(ssn: StateSeqNr) -> Self {
        match ssn {
            StateSeqNr::Invalid => INVALID_SSN,
            StateSeqNr::Value(SafeStateSeqNr(ssn)) => ssn.into(),
        }
    }
}

impl TryFrom<NonZeroU32> for SafeStateSeqNr {
    type Error = InvalidStateSeqNrError;

    fn try_from(value: NonZeroU32) -> Result<Self, Self::Error> {
        if u32::from(value) == RESERVED_SSN {
            Err(InvalidStateSeqNrError(RESERVED_SSN))
        } else {
            Ok(Self(value))
        }
    }
}

impl From<SafeStateSeqNr> for u32 {
    fn from(ssn: SafeStateSeqNr) -> Self {
        let SafeStateSeqNr(ssn) = ssn;
        u32::from(ssn)
    }
}

impl From<NonZeroU32> for StateSeqNr {
    fn from(value: NonZeroU32) -> Self {
        if u32::from(value) == RESERVED_SSN {
            Self::Invalid
        } else {
            Self::Value(value.try_into().expect("invalid cases should be covered"))
        }
    }
}

impl Add<u32> for StateSeqNr {
    type Output = StateSeqNr;

    fn add(self, rhs: u32) -> Self::Output {
        match self {
            Self::Invalid => Self::Invalid,
            Self::Value(ssn) => Self::Value(ssn + rhs),
        }
    }
}

impl AddAssign<u32> for StateSeqNr {
    fn add_assign(&mut self, rhs: u32) {
        match self {
            Self::Invalid => {}
            Self::Value(ssn) => *ssn = *ssn + rhs,
        }
    }
}

impl Add<u32> for SafeStateSeqNr {
    type Output = SafeStateSeqNr;

    /// Adds `rhs`, wrapping around from the largest valid value to the smallest one.
    fn add(self, rhs: u32) -> Self::Output {
        // valid values are 1..=RESERVED_SSN-1
        let count = u64::from(RESERVED_SSN - 1);
        let offset = (u64::from(u32::from(self)) - 1 + u64::from(rhs)) % count;
        let ssn = u32::try_from(offset + 1).expect("offset is smaller than the count");
        Self(NonZeroU32::new(ssn).expect("valid values start at 1"))
    }
}

impl StateSeqNr {
    /// Returns the [SafeStateSeqNr] if a valid state sequence number.
    pub fn value(self) -> Option<SafeStateSeqNr> {
        if let Self::Value(ssn) = self {
            Some(ssn)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use super::*;

    fn safe(value: u32) -> SafeStateSeqNr {
        SafeStateSeqNr::try_from(value).unwrap()
    }

    #[test]
    fn from_u32_maps_special_values() {
        assert_eq!(StateSeqNr::from(0u32), StateSeqNr::Invalid);
        assert_eq!(
            StateSeqNr::from(u32::MAX),
            StateSeqNr::Invalid,
            "reserved value"
        );
        assert_eq!(StateSeqNr::from(5u32), StateSeqNr::Value(safe(5)));
    }

    #[test]
    fn safe_try_from_rejects_special_values() {
        assert!(SafeStateSeqNr::try_from(0u32).is_err());
        assert!(SafeStateSeqNr::try_from(u32::MAX).is_err());
        assert_eq!(safe(1), SafeStateSeqNr::MIN);
        assert!(SafeStateSeqNr::try_from(u32::MAX - 1).is_ok());
    }

    #[test]
    fn safe_try_from_non_zero_rejects_reserved_value() {
        assert!(SafeStateSeqNr::try_from(NonZeroU32::MAX).is_err());
        assert!(SafeStateSeqNr::try_from(NonZeroU32::new(5).unwrap()).is_ok());
    }

    #[test]
    fn u32_round_trip() {
        assert_eq!(u32::from(StateSeqNr::Invalid), 0);
        assert_eq!(u32::from(StateSeqNr::Value(safe(5))), 5);
        assert_eq!(u32::from(safe(7)), 7);
    }

    #[test]
    fn from_non_zero_u32_maps_reserved_value_to_invalid() {
        assert_eq!(StateSeqNr::from(NonZeroU32::MAX), StateSeqNr::Invalid);
        assert_eq!(
            StateSeqNr::from(NonZeroU32::new(5).unwrap()),
            StateSeqNr::Value(safe(5))
        );
    }

    #[test]
    fn value() {
        assert!(StateSeqNr::Invalid.value().is_none());
        assert_eq!(StateSeqNr::from(5u32).value(), Some(safe(5)));
    }

    #[test]
    fn safe_state_seq_nr_is_ordered_numerically() {
        assert!(SafeStateSeqNr::MIN < safe(2));
        assert!(safe(2) < safe(3));
    }

    #[test]
    fn add_semantics() {
        // Invalid stays Invalid
        assert_eq!(StateSeqNr::Invalid + 5, StateSeqNr::Invalid);
        // Value is incremented
        assert_eq!(StateSeqNr::from(5u32) + 3, StateSeqNr::from(8u32));
    }

    #[test]
    fn add_assign_semantics() {
        let mut ssn = StateSeqNr::Invalid;
        ssn += 5;
        assert_eq!(ssn, StateSeqNr::Invalid, "Invalid must stay Invalid");

        let mut ssn = StateSeqNr::from(5u32);
        ssn += 3;
        assert_eq!(ssn, StateSeqNr::from(8u32));
    }

    #[test]
    fn add_wraps_around_to_the_smallest_value() {
        let max = safe(u32::MAX - 1);
        assert_eq!(max + 1, SafeStateSeqNr::MIN, "skips the reserved value");
        assert_eq!(max + 2, safe(2));
        assert_eq!(safe(5) + 0, safe(5));
        assert_eq!(safe(u32::MAX - 3) + 1, safe(u32::MAX - 2));
    }
}
