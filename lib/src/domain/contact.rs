use chrono::{DateTime, Utc};

use crate::domain::{Link, NodeId, Path};

/// Specifies in milliseconds how long ago the sender heard about the contact.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct Age(usize);

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct StateSeqNr(usize);

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct Timestamp(DateTime<Utc>);

impl From<DateTime<Utc>> for Timestamp {
    fn from(time: DateTime<Utc>) -> Self {
        Self(time)
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum State {
    Valid,
    Rediscovering,
    Invalid,
    Dead,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum RediscoveryState {
    None,
    Urgent(RediscoveryData),
    Regular(RediscoveryData),
    Slow(RediscoveryData),
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct RediscoveryData {
    failed_link: Link,
    time: DateTime<Utc>,
    failed_link_list: Vec<Link>,
    via_contacts: Vec<NodeId>,
    retry_counter: usize,
}

/// A Contact as represented in the RoutingTable.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Contact {
    state: State,
    age: Age,
    last_seen: Timestamp,
    path: Path,
    state_seq_nr: StateSeqNr,
}

impl Contact {
    pub fn new(age: Age, path: Path, state_seq_nr: StateSeqNr) -> Self {
        Self {
            state: State::Valid,
            age,
            last_seen: Timestamp::from(Utc::now()),
            path,
            state_seq_nr,
        }
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut State {
        &mut self.state
    }

    pub fn is_physical_neighbor(&self) -> bool {
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

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn path_mut(&mut self) -> &mut Path {
        &mut self.path
    }

    pub fn state_seq_nr(&self) -> &StateSeqNr {
        &self.state_seq_nr
    }

    pub fn state_seq_nr_mut(&mut self) -> &mut StateSeqNr {
        &mut self.state_seq_nr
    }
}
