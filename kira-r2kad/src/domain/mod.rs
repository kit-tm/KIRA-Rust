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
use std::hash::{Hash, Hasher};
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

use chrono::{DateTime, Duration, Utc};

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
pub struct Age(u64);

impl From<u64> for Age {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Display)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[display("{}", self.0.timestamp_millis())]
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

    /// Returns the [Timestamp] of the [Age].
    pub fn from_age(age: Age) -> Timestamp {
        Self(Utc::now() - Duration::milliseconds(age.0.cast_signed()))
    }
}

/// A underlay connection between two nodes.
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
#[display("NotVia {link} {age}ms")]
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
            age: notvia_state.timestamp.to_age(),
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

/// Data structure representing failed underlay connections with associated time information
/// This is for storing NotVia state internally
#[derive(Debug, Clone, Display)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[display("NotViaState {link} {timestamp}")]
pub struct NotViaState {
    pub link: Link,
    pub timestamp: Timestamp,
}

impl NotViaState {
    pub fn new(link: Link, timestamp: Timestamp) -> Self {
        Self { link, timestamp }
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
