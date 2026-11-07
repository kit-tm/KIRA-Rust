use std::{
    cmp::Ordering,
    hash::{
        Hash,
        Hasher,
    },
    time::Duration,
};

use derive_more::derive::Display;

use crate::{
    domain::{
        Age,
        NodeId,
        NotViaStateList,
        Path,
        PathCollection,
        SafeStateSeqNr,
        Timestamp,
    },
    use_cases::failure_handling::RediscoveryState,
};

#[derive(Debug, Clone, Eq, PartialEq, Display, Default)]
#[display("{_variant}")]
/// [ContactState] starts normally in Unknown for contacts heard from other nodes.
pub enum ContactState {
    #[default]
    /// When contact is initialized, its state is mostly unknown.
    Unknown,
    /// Has a validated active path.
    Valid,
    /// Active path is not valid due to failed links
    Invalid(NotViaStateList),
    /// No valid path, but trying to rediscover.
    Rediscovering(RediscoveryState),
    /// contact not usable anymore (e.g., rediscovery failed finally).
    Dead,
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

    pub fn set_state(&mut self, contact_state: ContactState) {
        self.state = contact_state;
    }

    pub fn start_rediscovering(
        &mut self,
        notviastate_list: NotViaStateList,
        via_contact_list: Vec<NodeId>,
        max_retries: u32,
        start_duration: Duration,
    ) {
        self.state = ContactState::Rediscovering(RediscoveryState::new(
            notviastate_list,
            via_contact_list,
            max_retries,
            start_duration,
        ));
    }

    /// Returns if the [Contact] represents an underlay neighbor (active path is direct link)
    pub fn is_uln(&self) -> bool {
        // FIXME: Invariant is, that path doesn't contain the own node_id. whole_path Method is for that.
        // However, one needs to distinguish whether the current contact is still a ULN but has
        // just lost its direct link...
        match self.path_collection.active_path() {
            Some(path) => (*path).size() == 1,
            None => false,
        }
    }

    /// Returns if the [Contact] has a failed ULN in its former active path
    pub fn has_broken_first_hop(&self) -> bool {
        match self.state {
            ContactState::Invalid(ref notvia_state_list) => {
                if let Some(path) = self.path_collection.active_path() {
                    notvia_state_list
                        .nvs_list
                        .iter()
                        .find(|nvs| nvs.link.contains(path.first()))
                        .is_some()
                } else {
                    false
                }
            }
            ContactState::Rediscovering(ref rds) => {
                if let Some(path) = self.path_collection.active_path() {
                    rds.get_notviastate_list()
                        .nvs_list
                        .iter()
                        .find(|nvs| nvs.link.contains(path.first()))
                        .is_some()
                } else {
                    false
                }
            }
            _ => false,
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

    pub fn is_dead(&self) -> bool {
        self.state == ContactState::Dead
    }

    pub fn is_valid(&self) -> bool {
        self.state == ContactState::Valid
    }

    pub fn is_invalid(&self) -> bool {
        matches!(self.state, ContactState::Invalid(_))
    }

    // sets contact state to invalid and current active path to invalid
    pub fn set_invalid(&mut self, notviastate_list: NotViaStateList) {
        // if contact is already in invalid or rediscovering, we may add an additional notvia link
        match self.state {
            ContactState::Invalid(ref mut existing_notviastate_list) => {
                for nvs in notviastate_list.nvs_list.iter() {
                    existing_notviastate_list.nvs_list.insert(nvs.clone());
                }
            }
            ContactState::Rediscovering(ref mut rds) => {
                for nvs in notviastate_list.nvs_list.iter() {
                    rds.add_notvia(nvs.into());
                }
            }
            _ => {
                // now set state to invalid
                self.state = ContactState::Invalid(notviastate_list.clone());
            }
        }
        // we use the latest timestamp of the given notviastate list to update last seen
        self.last_seen = *notviastate_list
            .nvs_list
            .iter()
            .map(|nvs| &nvs.timestamp)
            .max()
            .unwrap_or(&Timestamp::now());

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
            ContactState::Dead => false, // not sure that this is sensible
        };

        if path_suitable {
            // if path candidate has been validated (stems from a message's source route), we can also replace the active path directly
            if path_candidate.is_valid() {
                if let Some(proposed_path) = self.path_collection.proposed_path()
                    && path_candidate.is_same_path_as(proposed_path)
                {
                    self.path_collection.set_proposed_to_active();
                } else {
                    self.path_collection.set_active_path(path_candidate.clone());
                }
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

    /// Returns the active [Path] of the [Contact].
    pub fn into_path(self) -> Option<Path> {
        self.path_collection.into_active_path()
    }

    /// Returns the proposed [Path] of the [Contact].
    pub fn proposed_path(&self) -> Option<&Path> {
        self.path_collection.proposed_path()
    }

    pub fn proposed_path_mut(&mut self) -> Option<&mut Path> {
        self.path_collection.proposed_path_mut()
    }

    /// Returns the proposed [Path] of the [Contact].
    pub fn clear_proposed_path(&mut self) {
        self.path_collection.clear_proposed_path()
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

    pub fn set_state_seq_nr(&mut self, new_state_seq_nr: SafeStateSeqNr) {
        self.state_seq_nr = new_state_seq_nr;
    }

    pub fn update_state_seq_nr(&mut self, new_state_seq_nr: SafeStateSeqNr) -> bool {
        if self.state_seq_nr < new_state_seq_nr {
            self.state_seq_nr = new_state_seq_nr;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::domain::{
        Link,
        NodeId,
        NotViaState,
        Path,
        SafeStateSeqNr,
    };

    fn contact_with_path(path: &[NodeId], ssn: u32) -> Contact {
        Contact::new(
            Path::try_from(path).expect("test path must not be empty"),
            SafeStateSeqNr::try_from(ssn).expect("test ssn must be valid"),
        )
    }

    fn notvia_state_list(link: Link) -> NotViaStateList {
        NotViaStateList::from(NotViaState::new(link, Timestamp::now()))
    }

    #[test]
    fn dest_id_is_last_path_element() {
        let contact = contact_with_path(&[NodeId::from(1), NodeId::from(2), NodeId::from(3)], 1);
        assert_eq!(contact.id(), &NodeId::from(3));
    }

    #[test]
    fn new_contact_is_valid_with_active_path() {
        let contact = contact_with_path(&[NodeId::from(1), NodeId::from(2)], 1);
        assert!(contact.is_valid());
        assert!(!contact.is_invalid());
        assert!(!contact.is_dead());
        assert!(contact.state() == &ContactState::Valid);
        assert!(contact.path().is_some());
        assert!(contact.proposed_path().is_none());
    }

    #[test]
    fn cmp_actuality_prefers_greater_state_seq_nr() {
        let older_info = contact_with_path(&[NodeId::from(1)], 1);
        let mut newer_info = contact_with_path(&[NodeId::from(2)], 2);
        // make `newer_info` older in time to prove SSN dominates the ordering
        *newer_info.last_seen_mut() = Timestamp::from(Age::from(10_000u64));

        assert_eq!(
            newer_info.cmp_actuality(&older_info),
            Ordering::Greater,
            "greater SSN wins even when older"
        );
        assert_eq!(older_info.cmp_actuality(&newer_info), Ordering::Less);
        assert!(older_info.is_older_than(&newer_info));
        assert!(!newer_info.is_older_than(&older_info));
    }

    #[test]
    fn cmp_actuality_breaks_ties_with_recency() {
        // same SSN: the contact seen more recently carries the newer information
        let mut stale = contact_with_path(&[NodeId::from(1)], 1);
        *stale.last_seen_mut() = Timestamp::from(Age::from(10_000u64));
        let fresh = contact_with_path(&[NodeId::from(2)], 1);

        assert_eq!(fresh.cmp_actuality(&stale), Ordering::Greater);
        assert_eq!(stale.cmp_actuality(&fresh), Ordering::Less);
        assert!(stale.is_older_than(&fresh));
    }

    #[test]
    fn cmp_actuality_equal_information() {
        let a = contact_with_path(&[NodeId::from(1)], 1);
        let b = contact_with_path(&[NodeId::from(1)], 1);
        assert_eq!(a.cmp_actuality(&b), Ordering::Equal);
    }

    #[test]
    fn is_uln_only_for_single_hop_path() {
        let uln = contact_with_path(&[NodeId::from(1)], 1);
        assert!(uln.is_uln());

        let non_uln = contact_with_path(&[NodeId::from(1), NodeId::from(2)], 1);
        assert!(!non_uln.is_uln());
    }

    #[test]
    fn set_invalid_fresh_contact() {
        let mut contact = contact_with_path(&[NodeId::from(1), NodeId::from(2)], 1);
        let link = Link::new(NodeId::from(1), NodeId::from(9));

        contact.set_invalid(notvia_state_list(link.clone()));

        assert!(contact.is_invalid());
        let active_path = contact.path().expect("active path must exist");
        assert!(!active_path.is_valid(), "active path must be invalidated");
        assert!(active_path.is_faulty());
    }

    #[test]
    fn set_invalid_merges_notvia_links() {
        let mut contact = contact_with_path(&[NodeId::from(1), NodeId::from(2)], 1);
        let link_a = Link::new(NodeId::from(1), NodeId::from(9));
        let link_b = Link::new(NodeId::from(1), NodeId::from(8));

        contact.set_invalid(notvia_state_list(link_a.clone()));
        contact.set_invalid(notvia_state_list(link_b.clone()));

        assert!(contact.is_invalid());
        let ContactState::Invalid(nvs_list) = contact.state() else {
            panic!("contact should be invalid");
        };
        assert_eq!(nvs_list.nvs_list.len(), 2, "both links must be stored");
        assert!(nvs_list.nvs_list.contains(&NotViaState {
            link: link_a,
            timestamp: Timestamp::now(),
        }));
        assert!(nvs_list.nvs_list.contains(&NotViaState {
            link: link_b,
            timestamp: Timestamp::now(),
        }));
    }

    #[test]
    fn set_invalid_adds_links_while_rediscovering() {
        let mut contact = contact_with_path(&[NodeId::from(1), NodeId::from(2)], 1);
        let link_a = Link::new(NodeId::from(1), NodeId::from(9));
        let link_b = Link::new(NodeId::from(1), NodeId::from(8));

        contact.start_rediscovering(
            notvia_state_list(link_a.clone()),
            vec![],
            3,
            Duration::from_secs(1),
        );
        assert!(matches!(contact.state(), ContactState::Rediscovering(_)));

        contact.set_invalid(notvia_state_list(link_b.clone()));

        // must stay in Rediscovering state with both links recorded
        let ContactState::Rediscovering(rds) = contact.state() else {
            panic!("contact should still be rediscovering");
        };
        let nvs_list = rds.get_notviastate_list();
        assert_eq!(nvs_list.nvs_list.len(), 2, "both links must be stored");
        assert!(nvs_list.nvs_list.contains(&NotViaState {
            link: link_a,
            timestamp: Timestamp::now(),
        }));
        assert!(nvs_list.nvs_list.contains(&NotViaState {
            link: link_b,
            timestamp: Timestamp::now(),
        }));
    }

    #[test]
    fn has_broken_first_hop_invalid_state() {
        // path [1 -> 2 -> 3], broken link (2,9) does not affect first hop
        let mut contact =
            contact_with_path(&[NodeId::from(1), NodeId::from(2), NodeId::from(3)], 1);
        contact.set_invalid(notvia_state_list(Link::new(
            NodeId::from(2),
            NodeId::from(9),
        )));
        assert!(!contact.has_broken_first_hop());

        // broken link (1,2) contains the path's first hop
        let mut contact =
            contact_with_path(&[NodeId::from(1), NodeId::from(2), NodeId::from(3)], 1);
        contact.set_invalid(notvia_state_list(Link::new(
            NodeId::from(1),
            NodeId::from(2),
        )));
        assert!(contact.has_broken_first_hop());
    }

    #[test]
    fn has_broken_first_hop_rediscovering_state() {
        let mut contact = contact_with_path(&[NodeId::from(1), NodeId::from(2)], 1);
        contact.start_rediscovering(
            notvia_state_list(Link::new(NodeId::from(1), NodeId::from(2))),
            vec![],
            3,
            Duration::from_secs(1),
        );
        assert!(contact.has_broken_first_hop());
    }

    #[test]
    fn has_broken_first_hop_other_states() {
        let contact = contact_with_path(&[NodeId::from(1), NodeId::from(2)], 1);
        assert!(!contact.has_broken_first_hop(), "valid contact");

        let mut contact = contact_with_path(&[NodeId::from(1), NodeId::from(2)], 1);
        contact.set_state(ContactState::Dead);
        assert!(!contact.has_broken_first_hop(), "dead contact");
    }

    #[test]
    fn assess_candidate_shorter_path_updates_proposed_for_valid_contact() {
        let mut contact =
            contact_with_path(&[NodeId::from(1), NodeId::from(2), NodeId::from(3)], 1);
        // shorter, but not validated: only the proposed path may change
        let shorter = Path::try_from(&[NodeId::from(1), NodeId::from(3)][..]).unwrap();

        assert!(contact.assess_path_candidate_and_update(&shorter));
        assert_ne!(contact.path(), Some(&shorter), "active path unchanged");
        assert_eq!(contact.proposed_path(), Some(&shorter));
        assert!(contact.is_valid());
    }

    #[test]
    fn assess_candidate_longer_path_rejected_for_valid_contact() {
        let mut contact = contact_with_path(&[NodeId::from(1), NodeId::from(3)], 1);
        let longer =
            Path::try_from(&[NodeId::from(1), NodeId::from(2), NodeId::from(3)][..]).unwrap();

        assert!(!contact.assess_path_candidate_and_update(&longer));
        assert_eq!(contact.path().unwrap().size(), 2, "active path unchanged");
    }

    #[test]
    fn assess_candidate_same_as_proposed_promotes_proposed_to_active() {
        // For an invalid contact a validated candidate matching the proposed
        // path must promote the proposed path to the active path instead of
        // replacing it directly.
        let mut contact =
            contact_with_path(&[NodeId::from(1), NodeId::from(2), NodeId::from(3)], 1);
        contact.set_invalid(notvia_state_list(Link::new(
            NodeId::from(1),
            NodeId::from(9),
        )));
        let proposed =
            Path::try_from(&[NodeId::from(1), NodeId::from(2), NodeId::from(3)][..]).unwrap();
        contact.path_collection.set_proposed_path(proposed.clone());

        let mut validated = proposed.clone();
        validated.set_valid();

        assert!(contact.assess_path_candidate_and_update(&validated));
        assert!(contact.is_valid());
        let active = contact.path().expect("active path");
        assert!(active.is_valid(), "promoted path must be the validated one");
        assert!(active.is_same_path_as(&validated));
        assert!(
            contact.proposed_path().is_none(),
            "proposed path must be promoted to active"
        );
    }

    #[test]
    fn assess_candidate_unvalidated_updates_proposed_path_only() {
        let mut contact =
            contact_with_path(&[NodeId::from(1), NodeId::from(2), NodeId::from(3)], 1);
        // shorter than active but unvalidated: only the proposed path may change
        let candidate = Path::try_from(&[NodeId::from(1), NodeId::from(3)][..]).unwrap();

        assert!(contact.assess_path_candidate_and_update(&candidate));
        assert_ne!(contact.path(), Some(&candidate), "active path unchanged");
        assert_eq!(contact.proposed_path(), Some(&candidate));
        assert!(contact.is_valid());
    }

    #[test]
    fn assess_candidate_longer_path_rejected_for_proposed_path() {
        let mut contact = contact_with_path(&[NodeId::from(1), NodeId::from(3)], 1);
        let longer =
            Path::try_from(&[NodeId::from(1), NodeId::from(2), NodeId::from(3)][..]).unwrap();

        assert!(!contact.assess_path_candidate_and_update(&longer));
        assert!(contact.proposed_path().is_none());
    }

    #[test]
    fn assess_candidate_accepts_anything_for_invalid_contact() {
        let mut contact =
            contact_with_path(&[NodeId::from(1), NodeId::from(2), NodeId::from(3)], 1);
        contact.set_invalid(notvia_state_list(Link::new(
            NodeId::from(1),
            NodeId::from(9),
        )));

        // same length is fine, an invalid contact improves with any new candidate
        let candidate =
            Path::try_from(&[NodeId::from(1), NodeId::from(4), NodeId::from(3)][..]).unwrap();
        assert!(contact.assess_path_candidate_and_update(&candidate));
        assert_eq!(contact.proposed_path(), Some(&candidate));
    }

    #[test]
    fn assess_candidate_accepts_valid_candidate_for_invalid_contact() {
        let mut contact =
            contact_with_path(&[NodeId::from(1), NodeId::from(2), NodeId::from(3)], 1);
        contact.set_invalid(notvia_state_list(Link::new(
            NodeId::from(1),
            NodeId::from(9),
        )));

        let mut candidate = Path::try_from(&[NodeId::from(1), NodeId::from(3)][..]).unwrap();
        candidate.set_valid();

        assert!(contact.assess_path_candidate_and_update(&candidate));
        assert!(contact.is_valid(), "validated candidate restores validity");
        assert!(contact.path().unwrap().is_same_path_as(&candidate));
    }

    #[test]
    fn assess_candidate_rejected_for_dead_contact() {
        let mut contact = contact_with_path(&[NodeId::from(1), NodeId::from(3)], 1);
        contact.set_state(ContactState::Dead);

        let candidate =
            Path::try_from(&[NodeId::from(1), NodeId::from(2), NodeId::from(3)][..]).unwrap();
        assert!(!contact.assess_path_candidate_and_update(&candidate));
        assert!(contact.is_dead());
    }

    #[test]
    fn contact_hash_is_by_id() {
        use std::{
            collections::hash_map::DefaultHasher,
            hash::{
                Hash,
                Hasher,
            },
        };

        let a = contact_with_path(&[NodeId::from(1), NodeId::from(3)], 1);
        let b = contact_with_path(&[NodeId::from(9), NodeId::from(3)], 5);

        let mut hasher_a = DefaultHasher::new();
        let mut hasher_b = DefaultHasher::new();
        a.hash(&mut hasher_a);
        b.hash(&mut hasher_b);
        assert_eq!(hasher_a.finish(), hasher_b.finish(), "same id => same hash");
        assert_ne!(a, b);
    }
}
