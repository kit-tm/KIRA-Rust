//! Domain Layer of the KIRA software design.

pub use bucket::*;
pub use contact::*;
use derive_more::derive::Display;
pub use insertion_strategy::*;
pub use node_id::*;
pub use path::cycle_remover::*;
pub use path::in_order_cycle_remover::*;
pub use path::shortest_first_path_simplifier::*;
pub use path::simplifier::*;
pub use path::*;
pub use path_id::*;
pub use routing_table::flat_routing_table::*;
pub use routing_table::*;
pub use state_seq_nr::*;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};
pub use underlay::*;
pub use underlay_neighbor_table::*;
pub use vicinity::*;

pub mod bucket;
pub mod contact;
pub mod dht;
pub mod hasher;
pub mod insertion_strategy;
pub mod node_id;
pub mod path;
pub mod path_id;
pub mod protocol_event;
pub mod routing_table;
pub mod state_seq_nr;
pub mod underlay;
pub mod underlay_neighbor_table;
pub mod vicinity;

/// Specifies in milliseconds the age of the routing information.
///
/// This is either associated with the [Age] of a [Contact] or a failed link.
///
/// # Ordering
///
/// As [Age] specifies a timestamp in milliseconds a greater value represents a larger age.
/// Considering `X = Age(10)` and `Y = Age(20)` then `X < Y == true`.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Display)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[display("{}ms",self.0)]
pub struct Age(u64);

impl From<u64> for Age {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Display)]
#[display("{:?}", self.0)]
pub struct Timestamp(Instant);

impl From<Instant> for Timestamp {
    fn from(time: Instant) -> Self {
        Self(time)
    }
}

impl From<Timestamp> for Instant {
    fn from(ts: Timestamp) -> Self {
        ts.0
    }
}

impl Default for Timestamp {
    fn default() -> Self {
        Self::now()
    }
}

impl Timestamp {
    /// Creates a new Timestamp at current time.
    pub fn now() -> Self {
        Self(Instant::now()) // TODO replace with current_runtime::current_time()
    }

    /// Returns the [Age] of the [Timestamp].
    pub fn to_age(&self) -> Age {
        // OK since stored timestamp should always be >= current time
        Age::from(u64::try_from(self.0.elapsed().as_millis()).expect("Age value too large"))
    }

    /// Returns the [std::time::Duration] representation of the [Age] of the [Timestamp].
    pub fn to_age_duration(&self) -> Duration {
        self.0.elapsed()
    }

    /// Returns the [std::time::Duration] representation of the [Age] of the [Timestamp].
    pub fn to_age_duration_ms(&self) -> u64 {
        u64::try_from(self.0.elapsed().as_millis())
            .expect("age value should never exceed 64bit in ms")
    }
}

impl From<Age> for Timestamp {
    /// Returns the [Timestamp] of the [Age].
    fn from(age: Age) -> Timestamp {
        Timestamp::from(Timestamp::now().0 - (Duration::from_millis(age.0)))
    }
}

/// A underlay connection between two nodes,
/// stores an undirected Link
/// NOTE that the link uses a sorted order for the tuple for easier disambiguation
#[derive(Debug, Eq, PartialEq, Hash, Clone, Display)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[display("({_0}, {_1})")]
pub struct Link(NodeId, NodeId);

impl Link {
    pub fn new(first: NodeId, second: NodeId) -> Self {
        if first < second {
            Link(first, second)
        } else {
            Link(second, first)
        }
    }

    pub fn first(&self) -> &NodeId {
        &self.0
    }

    pub fn second(&self) -> &NodeId {
        &self.1
    }

    pub fn contains(&self, node_id: &NodeId) -> bool {
        self.0 == *node_id || self.1 == *node_id
    }
}

impl From<(NodeId, NodeId)> for Link {
    fn from((first, second): (NodeId, NodeId)) -> Self {
        Link::new(first, second)
    }
}

/// Data structure representing nodes or underlay connections to not use while forwarding protocol
/// messages.
///
/// Not via data is supposed to only represent information in the routing table.
/// It's an error for the local not via data to contain entries not affecting any nodes in the
/// routing table.
/// When a node gets deleted, all the not via data mentioning it will be removed.
#[derive(Debug, Clone, Display)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[display("NotVia {link} {age}")]
pub struct NotVia {
    pub link: Link,
    pub age: Age,
}

impl From<(Link, Age)> for NotVia {
    fn from((first, second): (Link, Age)) -> Self {
        Self {
            link: first,
            age: second,
        }
    }
}

impl From<&NotViaState> for NotVia {
    fn from(notvia_state: &NotViaState) -> Self {
        Self {
            link: notvia_state.link.clone(),
            age: Age::from(notvia_state.timestamp.to_age_duration_ms()),
        }
    }
}

impl Hash for NotVia {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.link.hash(state);
    }
}

// we ignore the Age component for checking the equality
impl PartialEq for NotVia {
    fn eq(&self, other: &Self) -> bool {
        self.link == other.link
    }
}

impl Eq for NotVia {}

pub type NotViaList = HashSet<NotVia>;

/// Data structure representing failed underlay connections with associated time information
/// This is for storing NotVia state internally
#[derive(Debug, Clone, Display)]
#[display("NotViaState {link} {timestamp:?}")]
pub struct NotViaState {
    pub link: Link,
    pub timestamp: Timestamp,
}

impl NotViaState {
    pub fn new(link: Link, timestamp: Timestamp) -> Self {
        Self { link, timestamp }
    }
}

impl From<NotVia> for NotViaState {
    fn from(not_via: NotVia) -> Self {
        Self {
            link: not_via.link,
            timestamp: Timestamp::from(not_via.age),
        }
    }
}

impl Hash for NotViaState {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.link.hash(state);
    }
}

// we ignore the timestamp component for checking the equality
impl PartialEq for NotViaState {
    fn eq(&self, other: &Self) -> bool {
        self.link == other.link
    }
}

impl Eq for NotViaState {}

#[derive(Debug, Clone, Eq, Display, PartialEq, Default)]
#[display("NotViaStateList {nvs_list:#?}")]
pub struct NotViaStateList {
    pub nvs_list: HashSet<NotViaState>,
}

impl From<HashSet<NotViaState>> for NotViaStateList {
    fn from(other_list: HashSet<NotViaState>) -> Self {
        Self {
            nvs_list: other_list,
        }
    }
}

impl From<NotViaState> for NotViaStateList {
    fn from(not_via_state: NotViaState) -> Self {
        Self {
            nvs_list: HashSet::from([not_via_state]),
        }
    }
}

impl From<NotViaStateList> for Option<NotViaList> {
    fn from(notviastatelist: NotViaStateList) -> Self {
        if notviastatelist.nvs_list.is_empty() {
            None
        } else {
            Some(notviastatelist.nvs_list.iter().map(NotVia::from).collect())
        }
    }
}

#[derive(Debug, Clone, Eq, Display, PartialEq)]
#[display("NotViaStateList {notviastate_list:#?} retries: {retry_counter}")]
pub struct RediscoveryState {
    notviastate_list: NotViaStateList, // any broken links within the active path
    rev_via_contact_list: Vec<NodeId>, // a list of NodeIds for contact (stored reversed so that we can pop)
    pub retry_counter: u8,
}

impl RediscoveryState {
    pub fn new(notviastate_list: NotViaStateList, via_contact_list: Vec<NodeId>) -> Self {
        Self {
            notviastate_list,
            rev_via_contact_list: via_contact_list.into_iter().rev().collect(),
            retry_counter: 0,
        }
    }

    // adds a notvia link to the list
    pub fn add_notvia(&mut self, not_via: NotVia) -> bool {
        self.notviastate_list.nvs_list.insert(not_via.into())
    }

    pub fn get_notviastate_list(&self) -> &NotViaStateList {
        &self.notviastate_list
    }

    // returns the via contact list in correct order (first element is next contact to try)
    pub fn get_via_contact_list(&self) -> Vec<NodeId> {
        self.rev_via_contact_list.iter().rev().cloned().collect()
    }

    // returns the via contact in XOR sorted order
    pub fn get_next_via_contact_id(&mut self) -> Option<NodeId> {
        self.rev_via_contact_list.pop()
    }
}
