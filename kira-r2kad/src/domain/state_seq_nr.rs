use derive_more::Display;
use std::ops::{Add, AddAssign};

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Display)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
/// Newtype for the underlay neighbor state used in the [ULNTable](crate::domain::ULNTable).
pub struct StateSeqNr(u64);

impl From<u64> for StateSeqNr {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl Add<u64> for StateSeqNr {
    type Output = StateSeqNr;

    fn add(self, rhs: u64) -> Self::Output {
        Self(self.0 + rhs)
    }
}

impl AddAssign<u64> for StateSeqNr {
    fn add_assign(&mut self, rhs: u64) {
        self.0 += rhs;
    }
}
