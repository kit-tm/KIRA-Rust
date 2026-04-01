use std::cmp::Ordering;

use chrono::Utc;
use derive_more::derive::Display;

use crate::domain::{Age, NodeId, Path, SafeStateSeqNr, Timestamp};

#[derive(Debug, Clone, Eq, PartialEq, Hash, Display)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[display("{_variant}")]
pub enum ContactState {
    Valid,         // has a validated active path
    Invalid,       // no valid path
    Rediscovering, // no valid path, but trying to rediscvoery
    Dead,          // contact not usable anymore (e.g., rediscovery failed finally)
}

/// A [Contact] as represented in the [RoutingTable](crate::domain::routing_table::RoutingTable).
/// A contact contains the destination NodeId, state information, and paths leading to the contact
/// The contact is Invalid if no valid paths are present
#[derive(Debug, Clone, Eq, PartialEq, Hash, Display)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[display("Contact [id: {}, age: {}, state_seq_nr: {state_seq_nr}, state: {state}, path: {path}]", self.id(), self.last_seen.to_age_duration())]
pub struct Contact {
    dest_id: NodeId,
    state: ContactState,
    last_seen: Timestamp,
    path: Path,
    state_seq_nr: SafeStateSeqNr,
}

impl Contact {
    /// Creates a new [Contact] with default values.
    ///
    /// The given [Path] has to end with the [NodeId] of the Contact.
    pub fn new(path: Path, state_seq_nr: SafeStateSeqNr) -> Self {
        Self {
            dest_id: *path.last(),
            state: ContactState::Valid,
            last_seen: Timestamp::from(Utc::now()),
            path,
            state_seq_nr,
        }
    }

    pub fn id(&self) -> &NodeId {
        &self.dest_id
    }

    pub fn into_id(self) -> NodeId {
        self.dest_id
    }

    pub fn state(&self) -> &ContactState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut ContactState {
        &mut self.state
    }

    /// Returns if the [Contact] represents a underlay neighbor.
    pub fn is_uln(&self) -> bool {
        // FIXME: Invariant is, that path doesn't contain the own node_id. whole_path Method is for that.
        // However, one needs to distinguish whether the current contact is still a ULN but has
        // just lost its direct link...
        self.path.size() == 1
    }

    pub fn age(&self) -> Age {
        self.last_seen.to_age()
    }

    /// Returns if the contact is older than the given contact `other`.
    pub fn is_older_than(&self, other: &Contact) -> bool {
        self.cmp_actuality(other) == Ordering::Less
    }

    /// Returns an [Ordering] based on the [SafeStateSeqNr] and [Age] of the contacts.
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

    pub fn state_seq_nr(&self) -> &SafeStateSeqNr {
        &self.state_seq_nr
    }

    pub fn state_seq_nr_mut(&mut self) -> &mut SafeStateSeqNr {
        &mut self.state_seq_nr
    }
}
