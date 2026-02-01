use derive_more::{Display, Error, From};
use std::{
    num::NonZeroU32,
    ops::{Add, AddAssign},
};

pub const INVALID_SSN: u32 = u32::MIN;
pub const RESET_SSN: u32 = u32::MAX;

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
