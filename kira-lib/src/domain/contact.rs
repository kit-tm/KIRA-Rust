use std::cmp::Ordering;
use std::fmt::{Display, Formatter};

use chrono::{DateTime, Duration, Utc};

use crate::domain::{NodeId, Path, StateSeqNr};

/// Specifies in milliseconds the age of the routing information.
///
/// This is either associated with the [Age] of a [Contact] or a failed link.
///
/// # Ordering
///
/// As [Age] specifies a timestamp in milliseconds a greater value represents a bigger age.
/// Considering `X = Age(10)` and `Y = Age(20)` then `X < Y == true`.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct Age(u64);

impl From<u64> for Age {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl Display for Age {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct Timestamp(
    #[cfg_attr(feature = "serde", serde(with = "chrono::serde::ts_milliseconds"))] DateTime<Utc>,
);

impl From<DateTime<Utc>> for Timestamp {
    fn from(time: DateTime<Utc>) -> Self {
        Self(time)
    }
}

impl Timestamp {
    /// Creates a new Timestamp at current time.
    pub fn now() -> Self {
        Self(Utc::now())
    }

    /// Returns the [Age] of the [Timestamp].
    pub fn to_age(&self) -> Age {
        let distance = Utc::now() - self.0;
        // OK since stored timestamp should always be >= current time
        Age::from(
            distance.num_seconds().unsigned_abs() * 1000
                + distance.num_milliseconds().unsigned_abs(),
        )
    }

    /// Returns the [Duration] representation of the [Age] of the [Timestamp].
    pub fn to_age_duration(&self) -> Duration {
        Utc::now() - self.0
    }
}

impl Display for Timestamp {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.timestamp_millis())
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum ContactState {
    Valid,
    Invalid,
}

impl Display for ContactState {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Valid => write!(f, "Valid"),
            Self::Invalid => write!(f, "Invalid"),
        }
    }
}

/// A [Contact] as represented in the [RoutingTable](crate::domain::routing_table::RoutingTable).
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct Contact {
    state: ContactState,
    last_seen: Timestamp,
    path: Path,
    state_seq_nr: StateSeqNr,
}

impl Display for Contact {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Contact [id: {}, age: {}, state_seq_nr: {}, state: {}, path: {}]",
            self.id(),
            self.last_seen.to_age_duration(),
            self.state_seq_nr,
            self.state,
            self.path
        )
    }
}

impl Contact {
    /// Creates a new [Contact] with default values.
    ///
    /// The given [Path] has to end with the [NodeId] of the Contact.
    pub fn new(path: Path, state_seq_nr: StateSeqNr) -> Self {
        Self {
            state: ContactState::Valid,
            last_seen: Timestamp::from(Utc::now()),
            path,
            state_seq_nr,
        }
    }

    pub fn id(&self) -> &NodeId {
        self.path.last()
    }

    pub fn into_id(self) -> NodeId {
        *self.path.last()
    }

    pub fn state(&self) -> &ContactState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut ContactState {
        &mut self.state
    }

    /// Returns if the [Contact] represents a underlay neighbor.
    pub fn is_pn(&self) -> bool {
        // FIXME: Invariant is, that path doesn't contain the own node_id. whole_path Method is for that.
        self.path.size() == 1
    }

    pub fn age(&self) -> Age {
        self.last_seen.to_age()
    }

    /// Returns if the contact is older than the given contact `other`.
    pub fn is_older_than(&self, other: &Contact) -> bool {
        self.cmp_actuality(other) == Ordering::Less
    }

    /// Returns an [Ordering] based on the [StateSeqNr] and [Age] of the contacts.
    ///
    /// - Greater: self has newer information.
    /// - Less: other has newer information.
    /// - Equals: Have the same information.
    pub fn cmp_actuality(&self, other: &Contact) -> Ordering {
        match (
            self.state_seq_nr.cmp(&other.state_seq_nr),
            self.age().cmp(&other.age()),
        ) {
            (Ordering::Greater, _) => Ordering::Greater,
            (Ordering::Less, _) => Ordering::Less,
            (Ordering::Equal, age_ordering) => age_ordering.reverse(),
        }
    }

    pub fn last_seen(&self) -> &Timestamp {
        &self.last_seen
    }

    pub fn set_last_seen_now(&mut self) {
        self.last_seen = Timestamp::from(Utc::now());
    }

    pub fn last_seen_mut(&mut self) -> &mut Timestamp {
        &mut self.last_seen
    }

    /// Returns the [Path] of the [Contact].
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns a mutable reference to the [Path] of the [Contact].
    ///
    /// As it's invalid for a [Path] not to end with the [NodeId] of the [Contact]
    /// and external users could violate this invariant, the access to the method
    /// is limited to the crate itself.
    pub(crate) fn path_mut(&mut self) -> &mut Path {
        &mut self.path
    }

    pub fn state_seq_nr(&self) -> &StateSeqNr {
        &self.state_seq_nr
    }

    pub fn state_seq_nr_mut(&mut self) -> &mut StateSeqNr {
        &mut self.state_seq_nr
    }
}
