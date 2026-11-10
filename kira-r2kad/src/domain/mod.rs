//! Domain Layer of the KIRA software design.

use std::{
    collections::HashSet,
    hash::Hash,
    time::{
        Duration,
        Instant,
    },
};

use derive_more::{
    Display,
    From,
};

pub mod bucket;
#[doc(inline)]
pub use bucket::{
    Bucket,
    DEFAULT_BUCKET_SIZE,
};
mod contact;
pub use contact::*;
mod contact_labels;
pub use contact_labels::*;

pub mod dht;

mod hasher;
pub use hasher::*;

pub mod insertion_strategy;
#[doc(inline)]
pub use insertion_strategy::InsertionStrategy;

pub mod protocol_message;
#[doc(inline)]
pub use protocol_message::{
    Nonce,
    ProtocolMessage,
    ProtocolMessageKind,
};

mod ip_prefixes;
pub use ip_prefixes::*;

mod node_id;
pub use node_id::*;

pub mod path;
#[doc(inline)]
pub use path::{
    Path,
    PathCollection,
    PathState,
};

mod path_id;
pub use path_id::*;

pub mod protocol_event;
pub use protocol_event::*;

pub mod routing_table;
#[doc(inline)]
pub use routing_table::RoutingTable;

mod source_route;
pub use source_route::*;

mod state_seq_nr;
pub use state_seq_nr::*;

mod underlay;
pub use underlay::*;

pub mod underlay_neighbor_table;
#[doc(inline)]
pub use underlay_neighbor_table::ULNTable;

pub mod vicinity;
#[doc(inline)]
pub use vicinity::{
    VICINITY_RADIUS,
    VicinityGraph,
};

/// Specifies in milliseconds the age of the routing information.
///
/// This is either associated with the [Age] of a [Contact] or a failed link.
///
/// # Ordering
///
/// As [Age] specifies a timestamp in milliseconds a greater value represents a larger age.
/// Considering `X = Age(10)` and `Y = Age(20)` then `X < Y == true`.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Display, From)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[display("{}ms",self.0)]
pub struct Age(u64);

impl Age {
    /// Returns the age increased by `duration`, e.g. by the estimated transit time.
    pub fn saturating_add(self, duration: Duration) -> Age {
        let ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
        Age(self.0.saturating_add(ms))
    }
}

impl From<Age> for std::time::Duration {
    fn from(Age(age_ms): Age) -> Self {
        std::time::Duration::from_millis(age_ms)
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

impl From<&mut Timestamp> for Instant {
    fn from(ts: &mut Timestamp) -> Self {
        ts.0
    }
}

impl Default for Timestamp {
    fn default() -> Self {
        Self::now()
    }
}

thread_local! {
    /// The current time of the event being handled on this thread, see [Timestamp::set_clock].
    static CLOCK: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
}

/// Restores the previous clock of the thread when dropped, see [Timestamp::set_clock].
#[must_use = "the clock is only set while the guard is alive"]
#[derive(Debug)]
pub struct ClockGuard {
    previous: Option<Instant>,
}

impl Drop for ClockGuard {
    fn drop(&mut self) {
        CLOCK.with(|clock| clock.set(self.previous));
    }
}

impl Timestamp {
    /// Sets the current time returned by [Timestamp::now] on this thread until the returned
    /// guard is dropped.
    ///
    /// The protocol never reads a clock itself. Whoever drives it (e.g. the event loop or a
    /// simulator with a virtual clock) provides the time of every event this way. Guards may
    /// be nested; dropping one restores the time that was set before.
    pub fn set_clock(now: Instant) -> ClockGuard {
        let previous = CLOCK.with(|clock| clock.replace(Some(now)));
        ClockGuard { previous }
    }

    /// Returns the current time as provided by [Timestamp::set_clock].
    ///
    /// # Panics
    ///
    /// If no clock is set on this thread, i.e. outside of handling an event.
    pub fn now() -> Self {
        let now = CLOCK.with(std::cell::Cell::get).expect(
            "no clock set: Timestamp::now() is only available while handling an event \
             (see Timestamp::set_clock)",
        );
        Self(now)
    }

    /// Returns the current time if a clock is set on this thread, see [Timestamp::now].
    pub fn try_now() -> Option<Self> {
        CLOCK.with(std::cell::Cell::get).map(Self)
    }

    /// Returns the age in ms of the [Timestamp] at the given time.
    pub fn age_at(&self, now: Timestamp) -> u64 {
        u64::try_from(now.0.saturating_duration_since(self.0).as_millis()).unwrap_or(u64::MAX)
    }

    /// Returns the [Age] of the [Timestamp].
    pub fn to_age(&self) -> Age {
        Age::from(self.to_age_duration_ms())
    }

    /// Returns the [std::time::Duration] representation of the [Age] of the [Timestamp].
    ///
    /// Timestamps from the future have no age.
    pub fn to_age_duration(&self) -> Duration {
        Self::now().0.saturating_duration_since(self.0)
    }

    /// Returns the [std::time::Duration] representation of the [Age] of the [Timestamp].
    pub fn to_age_duration_ms(&self) -> u64 {
        u64::try_from(self.to_age_duration().as_millis())
            .expect("age value should never exceed 64bit in ms")
    }
}

impl From<Age> for Timestamp {
    /// Returns the [Timestamp] of the [Age].
    fn from(age: Age) -> Timestamp {
        let now = Timestamp::now().0;
        let age = Duration::from_millis(age.0);
        // Ages are received from other nodes. On some platforms an Instant can't go back
        // further than the start of the monotonic clock, so saturate at the oldest
        // representable Instant instead of panicking.
        let timestamp = now.checked_sub(age).unwrap_or_else(|| {
            let (mut representable, mut too_old) = (Duration::ZERO, age);
            while too_old - representable > Duration::from_millis(1) {
                let mid = representable + (too_old - representable) / 2;
                if now.checked_sub(mid).is_some() {
                    representable = mid;
                } else {
                    too_old = mid;
                }
            }
            now - representable
        });
        Timestamp::from(timestamp)
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

impl NotVia {
    /// Returns the entry aged by `delay`.
    ///
    /// Used when forwarding failure information as own, e.g. echoed in a response: its age
    /// has to include the time since the original sender sent it.
    pub fn aged(&self, delay: Duration) -> Self {
        Self {
            link: self.link.clone(),
            age: self.age.saturating_add(delay),
        }
    }
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
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
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
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "no clock set")]
    fn now_without_clock_panics() {
        // the protocol must never fall back to reading a clock itself
        let _ = Timestamp::now();
    }

    #[test]
    fn clock_guards_restore_the_previous_time() {
        let t0 = Instant::now();
        let outer = Timestamp::set_clock(t0);
        {
            let _inner = Timestamp::set_clock(t0 + Duration::from_secs(5));
            assert_eq!(Instant::from(Timestamp::now()), t0 + Duration::from_secs(5));
        }
        assert_eq!(Instant::from(Timestamp::now()), t0);
        drop(outer);
        assert!(Timestamp::try_now().is_none());
    }

    #[test]
    fn ages_follow_the_provided_clock() {
        // e.g. the virtual clock of a simulator, independent of the wall clock
        let mut clock = crate::tests::TestClock::new();
        let timestamp = Timestamp::now();
        clock.advance(Duration::from_secs(3_600));
        assert_eq!(timestamp.to_age(), Age::from(3_600_000));
    }

    #[test]
    fn contact_display_without_clock_does_not_panic() {
        // log output may happen outside of handling an event
        let contact = {
            let _clock = crate::tests::TestClock::new();
            Contact::new(
                Path::from(NodeId::ONE),
                SafeStateSeqNr::try_from(1).unwrap(),
            )
        };
        assert!(contact.to_string().contains("age: ?"));
    }

    #[test]
    fn timestamp_from_age_lies_in_the_past() {
        let _clock = crate::tests::TestClock::new();
        let before = Timestamp::now();
        let timestamp = Timestamp::from(Age::from(10_000));
        assert!(timestamp < before);
        assert!(timestamp.to_age() >= Age::from(10_000));
    }

    #[test]
    fn timestamp_from_huge_age_does_not_panic() {
        let _clock = crate::tests::TestClock::new();
        // received ages are untrusted and may exceed what the platform's clock represents
        let timestamp = Timestamp::from(Age::from(u64::MAX));
        assert!(timestamp < Timestamp::now());
    }

    #[test]
    fn not_via_age_survives_conversion_to_state_and_back() {
        let _clock = crate::tests::TestClock::new();
        let link = Link::new(NodeId::ONE, NodeId::with_lsb(2));
        let state = NotViaState::from(NotVia::from((link, Age::from(5_000))));
        let age = NotVia::from(&state).age;
        assert!(
            age >= Age::from(5_000) && age < Age::from(6_000),
            "age {age}"
        );
    }
}
