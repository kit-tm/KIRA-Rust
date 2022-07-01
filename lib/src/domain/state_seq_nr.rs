use std::fmt::{Display, Formatter};
use std::ops::{Add, AddAssign};

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
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

impl Display for StateSeqNr {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
