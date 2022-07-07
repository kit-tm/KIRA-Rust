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
pub enum State {
    Valid,
    Rediscovering(RediscoveryState),
    Invalid,
    Dead,
}

impl Display for State {
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
///
/// The [Path] of a [Contact] is guaranteed to end with the [Contact]s
/// [NodeId].
#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct Contact {
    id: NodeId,
    state: State,
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
            self.id, self.path, self.age, self.last_seen, self.state_seq_nr, self.state
        )
    }
}

impl Contact {
    /// Creates a new [Contact] with default values.
    ///
    /// The [Path] is not allowed to end with the given [NodeId]
    /// for the [Contact] but won't be checked.
    pub fn new(id: NodeId, age: Age, path: Path, state_seq_nr: StateSeqNr) -> Self {
        assert!(path.last() != Some(&id));
        Self {
            id,
            state: State::Valid,
            age,
            last_seen: Timestamp::from(Utc::now()),
            path,
            state_seq_nr,
        }
    }

    pub fn id(&self) -> &NodeId {
        &self.id
    }

    pub fn into_id(self) -> NodeId {
        self.id
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut State {
        &mut self.state
    }

    /// Returns if the [Contact] represents a physical neighbor.
    pub fn is_pn(&self) -> bool {
        // FIXME: Invariant is, that path doesn't contain the own node_id. whole_path Method is for that.
        self.path.len() == 1
    }

    pub fn age(&self) -> &Age {
        &self.age
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

    /// Returns the [Path] to the [Contact] without the
    /// [NodeId] of the [Contact] itself as last element.
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn path_mut(&mut self) -> &mut Path {
        &mut self.path
    }

    /// Returns the [Path] to the [Contact] ending with the [NodeId]
    /// of the [Contact] itself.
    pub fn whole_path(&self) -> Path {
        let mut path = self.path.clone();
        path.push(self.id.clone());
        path
    }

    pub fn state_seq_nr(&self) -> &StateSeqNr {
        &self.state_seq_nr
    }

    pub fn state_seq_nr_mut(&mut self) -> &mut StateSeqNr {
        &mut self.state_seq_nr
    }
}
