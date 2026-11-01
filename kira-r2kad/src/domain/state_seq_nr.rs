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
const RESET_SSN: u32 = u32::MAX;

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Display, From)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(into = "u32", from = "u32"))]
/// Number that is monotonically increasing on underlay neighborhood changes of a node.
pub enum StateSeqNr {
    /// Not a valid [StateSeqNr].
    Invalid,
    /// The connection to the node should be reestablished.
    Reset,

    #[from]
    Value(SafeStateSeqNr),
}

/// Newtype that is guaranteed to be a valid state sequence number.
///
/// This also excludes the special sequence number `0xffffffff` signaling a reset.
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
            INVALID_SSN => Self::Invalid,
            RESET_SSN => Self::Reset,
            ssn => Self::Value(ssn.try_into().expect("invalid cases should be covered")),
        }
    }
}

impl TryFrom<u32> for SafeStateSeqNr {
    type Error = InvalidStateSeqNrError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            INVALID_SSN => Err(InvalidStateSeqNrError(INVALID_SSN)),
            RESET_SSN => Err(InvalidStateSeqNrError(RESET_SSN)),
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
            StateSeqNr::Reset => RESET_SSN,
        }
    }
}

impl TryFrom<NonZeroU32> for SafeStateSeqNr {
    type Error = InvalidStateSeqNrError;

    fn try_from(value: NonZeroU32) -> Result<Self, Self::Error> {
        if u32::from(value) == RESET_SSN {
            Err(InvalidStateSeqNrError(RESET_SSN))
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
        if u32::from(value) == RESET_SSN {
            Self::Reset
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
            Self::Value(ssn) => ssn + rhs,
            Self::Reset => rhs.into(),
        }
    }
}

impl AddAssign<u32> for StateSeqNr {
    fn add_assign(&mut self, rhs: u32) {
        match self {
            Self::Invalid => {}
            Self::Value(ssn) => {
                *self = *ssn + rhs;
            }
            Self::Reset => *self = rhs.into(),
        }
    }
}

impl Add<u32> for SafeStateSeqNr {
    type Output = StateSeqNr;

    fn add(self, rhs: u32) -> Self::Output {
        let Self(ssn) = self;

        // return Self::Reset on overflow
        ssn.saturating_add(rhs).into()
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

    pub fn is_reset(&self) -> bool {
        matches!(self, Self::Reset)
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use super::*;

    #[test]
    fn from_u32_maps_special_values() {
        assert_eq!(StateSeqNr::from(0u32), StateSeqNr::Invalid);
        assert_eq!(StateSeqNr::from(u32::MAX), StateSeqNr::Reset);
        assert_eq!(
            StateSeqNr::from(5u32),
            StateSeqNr::Value(SafeStateSeqNr::try_from(5).unwrap())
        );
    }

    #[test]
    fn safe_try_from_rejects_special_values() {
        assert!(SafeStateSeqNr::try_from(0u32).is_err());
        assert!(SafeStateSeqNr::try_from(u32::MAX).is_err());
        assert_eq!(
            SafeStateSeqNr::try_from(1u32).unwrap(),
            SafeStateSeqNr::MIN
        );
        assert!(SafeStateSeqNr::try_from(u32::MAX - 1).is_ok());
    }

    #[test]
    fn safe_try_from_non_zero_rejects_reset() {
        assert!(SafeStateSeqNr::try_from(NonZeroU32::MAX).is_err());
        assert!(SafeStateSeqNr::try_from(NonZeroU32::new(5).unwrap()).is_ok());
    }

    #[test]
    fn u32_round_trip() {
        assert_eq!(u32::from(StateSeqNr::Invalid), 0);
        assert_eq!(u32::from(StateSeqNr::Reset), u32::MAX);
        assert_eq!(
            u32::from(StateSeqNr::Value(SafeStateSeqNr::try_from(5).unwrap())),
            5
        );
        assert_eq!(
            u32::from(SafeStateSeqNr::try_from(7).unwrap()),
            7
        );
    }

    #[test]
    fn from_non_zero_u32_maps_reset() {
        assert_eq!(
            StateSeqNr::from(NonZeroU32::MAX),
            StateSeqNr::Reset,
            "the reset marker must not be a valid value"
        );
        assert_eq!(
            StateSeqNr::from(NonZeroU32::new(5).unwrap()),
            StateSeqNr::Value(SafeStateSeqNr::try_from(5).unwrap())
        );
    }

    #[test]
    fn value_and_is_reset() {
        assert!(StateSeqNr::Invalid.value().is_none());
        assert!(StateSeqNr::Reset.value().is_none());
        assert_eq!(
            StateSeqNr::from(5u32).value(),
            Some(SafeStateSeqNr::try_from(5).unwrap())
        );
        assert!(StateSeqNr::Reset.is_reset());
        assert!(!StateSeqNr::Invalid.is_reset());
        assert!(!StateSeqNr::from(5u32).is_reset());
    }

    #[test]
    fn safe_state_seq_nr_is_ordered_numerically() {
        assert!(SafeStateSeqNr::MIN < SafeStateSeqNr::try_from(2).unwrap());
        assert!(SafeStateSeqNr::try_from(2).unwrap() < SafeStateSeqNr::try_from(3).unwrap());
    }

    #[test]
    fn add_semantics() {
        // Invalid stays Invalid
        assert_eq!(StateSeqNr::Invalid + 5, StateSeqNr::Invalid);
        // Value is incremented
        assert_eq!(StateSeqNr::from(5u32) + 3, StateSeqNr::from(8u32));
        // Reset restarts counting from the addend
        assert_eq!(StateSeqNr::Reset + 5, StateSeqNr::from(5u32));
    }

    #[test]
    fn add_assign_semantics() {
        let mut ssn = StateSeqNr::Invalid;
        ssn += 5;
        assert_eq!(ssn, StateSeqNr::Invalid, "Invalid must stay Invalid");

        let mut ssn = StateSeqNr::from(5u32);
        ssn += 3;
        assert_eq!(ssn, StateSeqNr::from(8u32));

        let mut ssn = StateSeqNr::Reset;
        ssn += 5;
        assert_eq!(ssn, StateSeqNr::from(5u32), "Reset must restart counting");
    }

    #[test]
    fn add_wraps_to_reset_on_overflow() {
        let near_max = SafeStateSeqNr::try_from(u32::MAX - 1).unwrap();
        assert_eq!(near_max + 2, StateSeqNr::Reset, "overflow signals a reset");
        assert_eq!(
            near_max + 1,
            StateSeqNr::Reset,
            "saturating add to u32::MAX must map to Reset, not to a value"
        );
    }
}
