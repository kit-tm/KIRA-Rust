use std::cmp::Ordering;
use std::hash::{Hash, Hasher};

use derive_more::derive::Display;

use crate::domain::{Age, NodeId, NotViaStateList, Path, RediscoveryState, SafeStateSeqNr, Timestamp, pathcollection::PathCollection};

#[derive(Debug, Clone, Eq, PartialEq, Display, Default)]
#[display("{_variant}")]
/// [ContactState] starts normally in Unknown for contacts heard from other nodes.
///
pub enum ContactState {
    #[default]
    Unknown,       // when contact is initialized, its state is mostly unknown
    Valid,         // has a validated active path
    Invalid(NotViaStateList),   // active path is not valid due to failed links
    Rediscovering(RediscoveryState), // no valid path, but trying to rediscvoer
    Dead,          // contact not usable anymore (e.g., rediscovery failed finally)
}


/// A [Contact] as represented in the [RoutingTable](crate::domain::routing_table::RoutingTable).
/// A contact contains the destination NodeId, state information, and paths leading to the contact
/// The contact is Invalid if no valid paths are present
#[derive(Debug, Clone, Eq, PartialEq, Display)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[display("Contact [id: {}, age: {}ms, state_seq_nr: {state_seq_nr}, state: {state}, path: {path_collection:?}]", self.id(), self.last_seen.to_age_duration().as_millis())]
pub struct Contact {
    dest_id: NodeId,
    #[cfg_attr(feature = "serde", serde(skip))]
    state: ContactState,
    #[cfg_attr(feature = "serde", serde(skip))]
    last_seen: Timestamp,
    path_collection: PathCollection,
    state_seq_nr: SafeStateSeqNr,
}


impl Hash for Contact {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.dest_id.hash(state)
    }
}

impl Contact {
    /// Creates a new [Contact] with default values.
    ///
    /// The given [Path] has to end with the [NodeId] of the Contact.
    pub fn new(path: Path, state_seq_nr: SafeStateSeqNr) -> Self {
        Self {
            dest_id: *path.last(),
            state: ContactState::Valid,
            last_seen: Timestamp::now(),
            path_collection: PathCollection::new_with_active_path(path),
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

    pub fn start_rediscovering(&mut self, notviastate_list : NotViaStateList, via_contact_list : Vec<NodeId>) {
        self.state= ContactState::Rediscovering(RediscoveryState::new(notviastate_list, via_contact_list));
    }

    /// Returns if the [Contact] represents a underlay neighbor.
    pub fn is_uln(&self) -> bool {
        // FIXME: Invariant is, that path doesn't contain the own node_id. whole_path Method is for that.
        // However, one needs to distinguish whether the current contact is still a ULN but has
        // just lost its direct link...
        match self.path_collection.active_path() {
            Some(path) => (*path).size() == 1,
            None => false,
        }
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

    pub fn is_valid(&self) -> bool {
        self.state == ContactState::Valid
    }

    pub fn is_invalid(&self) -> bool {
        matches!(self.state,ContactState::Invalid(_))
    }

    // sets contact state to invalid and current active path to invalid
    pub fn set_invalid(&mut self, notviastate_list : NotViaStateList) {
        self.state = ContactState::Invalid(notviastate_list);

        if let Some(active_path) = self.path_mut() {
            active_path.invalidate();
        }
    }

    pub fn last_seen(&self) -> &Timestamp {
        &self.last_seen
    }

    pub fn set_last_seen_now(&mut self) {
        self.last_seen = Timestamp::now();
    }

    pub fn last_seen_mut(&mut self) -> &mut Timestamp {
        &mut self.last_seen
    }

    /// assesses whether given path candidate is an improvement over current active path
    /// if path_candidate is suitable this method updates the proposed path
    /// returns true if new path_candidate updated the proposed path or the active path (only when path_candidate was validated)
    pub fn assess_path_candidate_and_update(&mut self, path_candidate: &Path) -> bool {
        let path_suitable = match self.state {
            ContactState::Unknown => true,
            ContactState::Invalid(_) => true,
            ContactState::Valid => {
                let active_path = self
                    .path_collection
                    .active_path()
                    .expect("Valid contact should never have an unset active path");
                path_candidate.is_better_than(active_path)
            }
            ContactState::Rediscovering(_) => true,
            ContactState::Dead => false,
        };

        if path_suitable {
            // if path candidate has been validated (stems from a message's source route), we can also replace the active path directly
            if path_candidate.is_valid() {
                self.path_collection.set_active_path(path_candidate.clone());
                self.state = ContactState::Valid;
                return true;
            }
            // check if path_candidate is better than current proposed path if present
            let should_set_proposed_path = match self.path_collection.proposed_path() {
                Some(current_proposed_path) => path_candidate.is_better_than(current_proposed_path), // will return true if path candidate is better than current proposed path
                None => true,
            };

            if should_set_proposed_path {
                // set a new proposed path
                self.path_collection
                    .set_proposed_path(path_candidate.clone());
                return true;
            }
        }
        false
    }

    /// Returns the active [Path] of the [Contact].
    pub fn path(&self) -> Option<&Path> {
        self.path_collection.active_path()
    }

    /// Returns the proposed [Path] of the [Contact].
    pub fn proposed_path(&self) -> Option<&Path> {
        self.path_collection.proposed_path()
    }

    /// Set the active [Path] of the [Contact].
    pub fn set_path(&mut self, new_path: Path) {
        self.path_collection.set_active_path(new_path);
    }

    /// Returns a mutable reference to the [Path] of the [Contact].
    ///
    /// As it's invalid for a [Path] not to end with the [NodeId] of the [Contact]
    /// and external users could violate this invariant, the access to the method
    /// is limited to the crate itself.
    pub(crate) fn path_mut(&mut self) -> Option<&mut Path> {
        self.path_collection.active_path_mut()
    }

    pub fn state_seq_nr(&self) -> &SafeStateSeqNr {
        &self.state_seq_nr
    }

    pub fn state_seq_nr_mut(&mut self) -> &mut SafeStateSeqNr {
        &mut self.state_seq_nr
    }
}
