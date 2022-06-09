use chrono::{DateTime, Utc};

use crate::domain::{Link, NodeId, Path, DEFAULT_ID_SIZE};

/// Specifies in milliseconds how long ago the sender heard about the contact.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct Age(usize);

impl From<usize> for Age {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct StateSeqNr(usize);

impl From<usize> for StateSeqNr {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

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
pub enum RediscoveryState<const ID_SIZE: usize = DEFAULT_ID_SIZE> {
    None,
    Urgent(RediscoveryData<ID_SIZE>),
    Regular(RediscoveryData<ID_SIZE>),
    Slow(RediscoveryData<ID_SIZE>),
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct RediscoveryData<const ID_SIZE: usize = DEFAULT_ID_SIZE> {
    failed_link: Link<ID_SIZE>,
    time: Timestamp,
    failed_link_list: Vec<Link<ID_SIZE>>,
    via_contacts: Vec<NodeId<ID_SIZE>>,
    retry_counter: usize,
}

/// A Contact as represented in the RoutingTable.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct Contact<const ID_SIZE: usize = DEFAULT_ID_SIZE> {
    id: NodeId<ID_SIZE>,
    state: State,
    age: Age,
    last_seen: Timestamp,
    path: Path<ID_SIZE>,
    state_seq_nr: StateSeqNr,
    redisc_state: RediscoveryState,
}

impl<const ID_SIZE: usize> Contact<ID_SIZE> {
    pub fn new(
        id: NodeId<ID_SIZE>,
        age: Age,
        path: Path<ID_SIZE>,
        state_seq_nr: StateSeqNr,
    ) -> Self {
        Self {
            id,
            state: State::Valid,
            age,
            last_seen: Timestamp::from(Utc::now()),
            path,
            state_seq_nr,
            redisc_state: RediscoveryState::None,
        }
    }

    pub fn id(&self) -> &NodeId<ID_SIZE> {
        &self.id
    }

    pub fn into_id(self) -> NodeId<ID_SIZE> {
        self.id
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

    pub fn path(&self) -> &Path<ID_SIZE> {
        &self.path
    }

    pub fn path_mut(&mut self) -> &mut Path<ID_SIZE> {
        &mut self.path
    }

    pub fn state_seq_nr(&self) -> &StateSeqNr {
        &self.state_seq_nr
    }

    pub fn state_seq_nr_mut(&mut self) -> &mut StateSeqNr {
        &mut self.state_seq_nr
    }

    pub fn rediscovery_state(&self) -> &RediscoveryState {
        &self.redisc_state
    }

    pub fn rediscovery_state_mut(&mut self) -> &mut RediscoveryState {
        &mut self.redisc_state
    }
}
