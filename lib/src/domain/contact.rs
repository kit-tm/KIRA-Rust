use std::fmt::{Display, Formatter};

use chrono::{DateTime, Utc};

use crate::domain::{Link, NodeId, Path, StateSeqNr};

/// Specifies in milliseconds how long ago the sender heard about the contact.
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

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct Timestamp(
    #[cfg_attr(feature = "serde", serde(with = "chrono::serde::ts_milliseconds"))] DateTime<Utc>,
);

impl From<DateTime<Utc>> for Timestamp {
    fn from(time: DateTime<Utc>) -> Self {
        Self(time)
    }
}

impl Display for Timestamp {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.timestamp_millis())
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum ContactState {
    Valid,
    Rediscovering(RediscoveryState),
    Invalid,
    Dead,
}

impl Display for ContactState {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Valid => write!(f, "Valid"),
            Self::Invalid => write!(f, "Invalid"),
            Self::Dead => write!(f, "Dead"),
            Self::Rediscovering(_) => write!(f, "Rediscovering"),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum RediscoveryType {
    Urgent,
    Regular,
    Slow,
}

#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct RediscoveryState {
    pub typ: RediscoveryType,
    pub time: Timestamp,
    pub failed_link_list: Vec<Link>,
    pub via_contacts: Vec<NodeId>,
    pub retry_counter: usize,
}

/// A [Contact] as represented in the [RoutingTable].
#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct Contact {
    state: ContactState,
    age: Age,
    last_seen: Timestamp,
    path: Path,
    state_seq_nr: StateSeqNr,
}

impl Display for Contact {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Contact#{} {} [age: {}, last_seen: {}, state_seq_nr: {}, state: {}]",
            self.id(),
            self.path,
            self.age,
            self.last_seen,
            self.state_seq_nr,
            self.state
        )
    }
}

impl Contact {
    /// Creates a new [Contact] with default values.
    ///
    /// The given [Path] has to end with the [NodeId] of the Contact.
    pub fn new(path: Path, age: Age, state_seq_nr: StateSeqNr) -> Self {
        Self {
            state: ContactState::Valid,
            age,
            last_seen: Timestamp::from(Utc::now()),
            path,
            state_seq_nr,
        }
    }

    pub fn id(&self) -> &NodeId {
        self.path.last()
    }

    pub fn into_id(self) -> NodeId {
        self.path.last().clone()
    }

    pub fn state(&self) -> &ContactState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut ContactState {
        &mut self.state
    }

    /// Returns if the [Contact] represents a physical neighbor.
    pub fn is_pn(&self) -> bool {
        // FIXME: Invariant is, that path doesn't contain the own node_id. whole_path Method is for that.
        self.path.size() == 1
    }

    pub fn age(&self) -> &Age {
        &self.age
    }

    pub fn age_mut(&mut self) -> &mut Age {
        &mut self.age
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
