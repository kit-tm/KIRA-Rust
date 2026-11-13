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
        ContactLabels,
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
#[display("Contact [id: {}, age: {}, state_seq_nr: {state_seq_nr}, state: {state}, path: {path_collection:?}]", self.id(), self.display_age())]
pub struct Contact {
    dest_id: NodeId,
    #[cfg_attr(feature = "serde", serde(skip))]
    state: ContactState,
    #[cfg_attr(feature = "serde", serde(skip))]
    last_seen: Timestamp,
    path_collection: PathCollection,
    state_seq_nr: SafeStateSeqNr,
    /// Labels set by this node, see [ContactLabels::LOCAL].
    #[cfg_attr(feature = "serde", serde(skip))]
    local_labels: ContactLabels,
    /// Labels advertised by other nodes, see [ContactLabels::ADVERTISED].
    ///
    /// `None` means no information was received, so existing labels are kept on merge.
    #[cfg_attr(feature = "serde", serde(skip))]
    advertised_labels: Option<ContactLabels>,
    /// When a lower [SafeStateSeqNr] of the node was accepted, e.g., after its restart.
    ///
    /// Information dated before is outdated, whatever its sequence number.
    #[cfg_attr(feature = "serde", serde(skip))]
    ssn_reset_at: Option<Timestamp>,
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
            local_labels: ContactLabels::empty(),
            advertised_labels: None,
            ssn_reset_at: None,
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

    /// Returns all labels of the [Contact]: derived, local and advertised ones.
    pub fn labels(&self) -> ContactLabels {
        let mut labels = self.local_labels | self.advertised_labels.unwrap_or_default();
        labels.set(ContactLabels::Uln, self.is_uln());
        labels
    }

    /// Returns if the [Contact] has at least one of the given labels.
    pub fn has_any_label(&self, labels: ContactLabels) -> bool {
        self.labels().intersects(labels)
    }

    pub fn local_labels(&self) -> ContactLabels {
        self.local_labels
    }

    /// Adds local labels. Labels from other sources are ignored.
    pub fn add_local_labels(&mut self, labels: ContactLabels) {
        self.local_labels |= labels & ContactLabels::LOCAL;
    }

    /// Removes local labels. Labels from other sources are ignored.
    pub fn remove_local_labels(&mut self, labels: ContactLabels) {
        self.local_labels -= labels & ContactLabels::LOCAL;
    }

    pub fn advertised_labels(&self) -> Option<ContactLabels> {
        self.advertised_labels
    }

    /// Sets the advertised labels. Labels from other sources are ignored.
    ///
    /// `None` means that no label information is available.
    pub fn set_advertised_labels(&mut self, labels: Option<ContactLabels>) {
        self.advertised_labels = labels.map(|labels| labels & ContactLabels::ADVERTISED);
    }

    /// Merges labels from `newer`, which carries more recent information about the same node.
    ///
    /// Use this when the stored contact is kept and updated with received information.
    ///
    /// * Local labels are combined, as they're only changed explicitly.
    /// * Advertised labels are taken from `newer`, unless it has none (`None`).
    pub fn update_labels_from(&mut self, newer: &Contact) {
        debug_assert_eq!(self.id(), newer.id(), "merging labels of different nodes");
        self.local_labels |= newer.local_labels;
        self.advertised_labels = newer.advertised_labels.or(self.advertised_labels);
    }

    /// Takes over labels from `previous`, the stored contact this contact replaces.
    ///
    /// Use this when a contact is moved or re-inserted, so no labels get lost.
    ///
    /// * Local labels are combined, as they're only changed explicitly.
    /// * Advertised labels are kept, unless this contact has none (`None`).
    pub fn inherit_labels_from(&mut self, previous: &Contact) {
        debug_assert_eq!(
            self.id(),
            previous.id(),
            "merging labels of different nodes"
        );
        self.local_labels |= previous.local_labels;
        self.advertised_labels = self.advertised_labels.or(previous.advertised_labels);
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

    /// Age for log output, which must never panic: unknown outside of handling an event.
    fn display_age(&self) -> String {
        Timestamp::try_now().map_or_else(
            || "?".to_string(),
            |now| format!("{}", Age::from(self.last_seen.age_at(now))),
        )
    }

    pub fn age(&self) -> Age {
        self.last_seen.to_age()
    }

    /// Smoothed round-trip time to the contact over its active path, if measured.
    pub fn srtt(&self) -> Option<Duration> {
        self.path().and_then(Path::srtt)
    }

    /// Estimated one-way delay to the contact: half of the smoothed round-trip time.
    pub fn one_way_delay(&self) -> Option<Duration> {
        self.srtt().map(|srtt| srtt / 2)
    }

    /// Age of the routing information about this contact, as reported to other nodes in
    /// RTable objects.
    ///
    /// It is 0 for valid underlay neighbors, as information about own links is always current.
    /// Otherwise it is the time since the active path was validated or, without validation,
    /// since the contact was last seen.
    pub fn info_age(&self) -> Age {
        if self.is_uln() && self.is_valid() {
            return Age::from(0);
        }
        self.path()
            .and_then(Path::get_last_validated)
            .map_or_else(|| self.age(), Timestamp::to_age)
    }

    /// Dates the contact information back by the age reported by another node
    /// (`t_j := t_now - ∆T_j`), so it compares correctly with own information.
    pub fn set_info_age(&mut self, age: Age) {
        self.last_seen = Timestamp::from(age);
    }

    /// Returns if the contact is older than the given contact `other`.
    pub fn is_older_than(&self, other: &Contact) -> bool {
        self.cmp_actuality(other) == Ordering::Less
    }

    /// Returns an [Ordering] based on the [SafeStateSeqNr] and [Age] of the contacts.
    ///
    /// Information dated before a known reset of the node's sequence number is older,
    /// whatever its sequence number.
    ///
    /// - Greater: self has newer information.
    /// - Less: other has newer information.
    /// - Equals: Have the same information.
    pub fn cmp_actuality(&self, other: &Contact) -> Ordering {
        let self_predates_reset = other
            .ssn_reset_at
            .is_some_and(|reset| self.last_seen < reset);
        let other_predates_reset = self
            .ssn_reset_at
            .is_some_and(|reset| other.last_seen < reset);
        match (self_predates_reset, other_predates_reset) {
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            _ => {}
        }
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

    /// Refreshes when the node was last heard of, if `seen` is later.
    pub fn refresh_last_seen(&mut self, seen: Timestamp) {
        if seen > self.last_seen {
            self.last_seen = seen;
        }
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
                    // a proposed path is only worth probing if it improves the new active path
                    if self
                        .path_collection
                        .proposed_path()
                        .is_some_and(|proposed| !proposed.is_better_than(path_candidate))
                    {
                        self.path_collection.clear_proposed_path();
                    }
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

    /// Accepts a lower sequence number from an authoritative answer of the node
    /// (e.g., after its restart) and remembers when.
    pub fn reset_state_seq_nr(&mut self, new_state_seq_nr: SafeStateSeqNr) {
        self.state_seq_nr = new_state_seq_nr;
        self.ssn_reset_at = Some(Timestamp::now());
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
        let _clock = crate::tests::TestClock::new();
        let contact = contact_with_path(&[NodeId::from(1), NodeId::from(2), NodeId::from(3)], 1);
        assert_eq!(contact.id(), &NodeId::from(3));
    }

    #[test]
    fn new_contact_is_valid_with_active_path() {
        let _clock = crate::tests::TestClock::new();
        let contact = contact_with_path(&[NodeId::from(1), NodeId::from(2)], 1);
        assert!(contact.is_valid());
        assert!(!contact.is_invalid());
        assert!(!contact.is_dead());
        assert!(contact.state() == &ContactState::Valid);
        assert!(contact.path().is_some());
        assert!(contact.proposed_path().is_none());
    }

    fn ssn(value: u32) -> SafeStateSeqNr {
        SafeStateSeqNr::try_from(value).unwrap()
    }

    #[test]
    fn information_from_before_a_reset_is_older_whatever_its_ssn() {
        let mut clock = crate::tests::TestClock::new();
        let mut known = contact_with_path(&[NodeId::from(1), NodeId::from(2)], 10);
        clock.advance(std::time::Duration::from_secs(5));
        // the node restarted and its answer was accepted
        known.reset_state_seq_nr(ssn(2));

        // relayed by a node that still knows the node from before the restart (3s ago)
        let mut stale = contact_with_path(&[NodeId::from(3), NodeId::from(2)], 9);
        stale.set_info_age(Age::from(3_000));

        assert!(stale.is_older_than(&known));
        assert!(!known.is_older_than(&stale));
    }

    #[test]
    fn information_after_a_reset_compares_by_ssn() {
        let mut clock = crate::tests::TestClock::new();
        let mut known = contact_with_path(&[NodeId::from(1), NodeId::from(2)], 10);
        known.reset_state_seq_nr(ssn(2));
        clock.advance(std::time::Duration::from_secs(1));

        let newer = contact_with_path(&[NodeId::from(3), NodeId::from(2)], 3);
        let outdated = contact_with_path(&[NodeId::from(3), NodeId::from(2)], 1);

        assert!(known.is_older_than(&newer));
        assert!(outdated.is_older_than(&known));
    }

    #[test]
    fn cmp_actuality_prefers_greater_state_seq_nr() {
        let _clock = crate::tests::TestClock::new();
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
        let _clock = crate::tests::TestClock::new();
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
        let _clock = crate::tests::TestClock::new();
        let a = contact_with_path(&[NodeId::from(1)], 1);
        let b = contact_with_path(&[NodeId::from(1)], 1);
        assert_eq!(a.cmp_actuality(&b), Ordering::Equal);
    }

    #[test]
    fn is_uln_only_for_single_hop_path() {
        let _clock = crate::tests::TestClock::new();
        let uln = contact_with_path(&[NodeId::from(1)], 1);
        assert!(uln.is_uln());

        let non_uln = contact_with_path(&[NodeId::from(1), NodeId::from(2)], 1);
        assert!(!non_uln.is_uln());
    }

    #[test]
    fn set_invalid_fresh_contact() {
        let _clock = crate::tests::TestClock::new();
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
        let _clock = crate::tests::TestClock::new();
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
        let _clock = crate::tests::TestClock::new();
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
        let _clock = crate::tests::TestClock::new();
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
        let _clock = crate::tests::TestClock::new();
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
        let _clock = crate::tests::TestClock::new();
        let contact = contact_with_path(&[NodeId::from(1), NodeId::from(2)], 1);
        assert!(!contact.has_broken_first_hop(), "valid contact");

        let mut contact = contact_with_path(&[NodeId::from(1), NodeId::from(2)], 1);
        contact.set_state(ContactState::Dead);
        assert!(!contact.has_broken_first_hop(), "dead contact");
    }

    #[test]
    fn assess_candidate_shorter_path_updates_proposed_for_valid_contact() {
        let _clock = crate::tests::TestClock::new();
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
        let _clock = crate::tests::TestClock::new();
        let mut contact = contact_with_path(&[NodeId::from(1), NodeId::from(3)], 1);
        let longer =
            Path::try_from(&[NodeId::from(1), NodeId::from(2), NodeId::from(3)][..]).unwrap();

        assert!(!contact.assess_path_candidate_and_update(&longer));
        assert_eq!(contact.path().unwrap().size(), 2, "active path unchanged");
    }

    #[test]
    fn assess_candidate_same_as_proposed_promotes_proposed_to_active() {
        let _clock = crate::tests::TestClock::new();
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
        let _clock = crate::tests::TestClock::new();
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
        let _clock = crate::tests::TestClock::new();
        let mut contact = contact_with_path(&[NodeId::from(1), NodeId::from(3)], 1);
        let longer =
            Path::try_from(&[NodeId::from(1), NodeId::from(2), NodeId::from(3)][..]).unwrap();

        assert!(!contact.assess_path_candidate_and_update(&longer));
        assert!(contact.proposed_path().is_none());
    }

    /// Invalid contact to node 5 with the given proposed path.
    fn invalid_contact_with_proposed(proposed: &[u128]) -> Contact {
        let ids = |nodes: &[u128]| nodes.iter().map(|n| NodeId::from(*n)).collect::<Vec<_>>();
        let mut contact = contact_with_path(&ids(&[1, 2, 3, 4, 5]), 1);
        contact.set_invalid(notvia_state_list(Link::new(
            NodeId::from(1),
            NodeId::from(2),
        )));
        contact
            .path_collection
            .set_proposed_path(Path::try_from(&ids(proposed)[..]).unwrap());
        contact
    }

    fn validated(nodes: &[u128]) -> Path {
        let ids = nodes.iter().map(|n| NodeId::from(*n)).collect::<Vec<_>>();
        let mut path = Path::try_from(&ids[..]).unwrap();
        path.set_valid();
        path
    }

    #[test]
    fn proposed_path_not_better_than_new_active_path_is_dropped() {
        let _clock = crate::tests::TestClock::new();
        let mut contact = invalid_contact_with_proposed(&[6, 7, 5]);

        assert!(contact.assess_path_candidate_and_update(&validated(&[8, 5])));

        assert!(contact.path().unwrap().is_same_path_as(&validated(&[8, 5])));
        assert!(contact.proposed_path().is_none());
    }

    #[test]
    fn proposed_path_better_than_new_active_path_is_kept() {
        let _clock = crate::tests::TestClock::new();
        let mut contact = invalid_contact_with_proposed(&[6, 5]);

        assert!(contact.assess_path_candidate_and_update(&validated(&[8, 9, 5])));

        assert!(
            contact
                .path()
                .unwrap()
                .is_same_path_as(&validated(&[8, 9, 5]))
        );
        assert_eq!(
            contact.proposed_path().unwrap().size(),
            2,
            "still worth probing"
        );
    }

    #[test]
    fn assess_candidate_accepts_anything_for_invalid_contact() {
        let _clock = crate::tests::TestClock::new();
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
        let _clock = crate::tests::TestClock::new();
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
        let _clock = crate::tests::TestClock::new();
        let mut contact = contact_with_path(&[NodeId::from(1), NodeId::from(3)], 1);
        contact.set_state(ContactState::Dead);

        let candidate =
            Path::try_from(&[NodeId::from(1), NodeId::from(2), NodeId::from(3)][..]).unwrap();
        assert!(!contact.assess_path_candidate_and_update(&candidate));
        assert!(contact.is_dead());
    }

    #[test]
    fn contact_hash_is_by_id() {
        let _clock = crate::tests::TestClock::new();
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

    #[test]
    fn uln_label_is_derived_from_path() {
        let _clock = crate::tests::TestClock::new();
        let uln = contact_with_path(&[NodeId::from(1)], 1);
        assert_eq!(uln.labels(), ContactLabels::Uln);

        let non_uln = contact_with_path(&[NodeId::from(1), NodeId::from(2)], 1);
        assert_eq!(non_uln.labels(), ContactLabels::empty());
    }

    #[test]
    fn labels_combine_all_sources() {
        let _clock = crate::tests::TestClock::new();
        let mut contact = contact_with_path(&[NodeId::from(1)], 1);
        contact.add_local_labels(ContactLabels::Cached);
        contact.set_advertised_labels(Some(ContactLabels::Landmark));

        assert_eq!(
            contact.labels(),
            ContactLabels::Uln | ContactLabels::Cached | ContactLabels::Landmark
        );
        assert!(contact.has_any_label(ContactLabels::Landmark | ContactLabels::EndSystem));
        assert!(!contact.has_any_label(ContactLabels::EndSystem));
    }

    #[test]
    fn labels_from_wrong_source_are_ignored() {
        let _clock = crate::tests::TestClock::new();
        let mut contact = contact_with_path(&[NodeId::from(1), NodeId::from(2)], 1);

        contact.add_local_labels(ContactLabels::all());
        assert_eq!(contact.local_labels(), ContactLabels::LOCAL);

        contact.set_advertised_labels(Some(ContactLabels::all()));
        assert_eq!(contact.advertised_labels(), Some(ContactLabels::ADVERTISED));

        contact.remove_local_labels(ContactLabels::all());
        assert_eq!(contact.labels(), ContactLabels::ADVERTISED);
    }

    #[test]
    fn update_labels_from_newer() {
        let _clock = crate::tests::TestClock::new();
        let path = [NodeId::from(1), NodeId::from(2)];
        let mut stored = contact_with_path(&path, 1);
        stored.add_local_labels(ContactLabels::Cached);
        stored.set_advertised_labels(Some(ContactLabels::Landmark));

        // newer without label information keeps everything
        stored.update_labels_from(&contact_with_path(&path, 2));
        assert_eq!(
            stored.labels(),
            ContactLabels::Cached | ContactLabels::Landmark
        );

        // newer advertised labels replace the stored ones, local labels stay
        let mut newer = contact_with_path(&path, 3);
        newer.set_advertised_labels(Some(ContactLabels::EndSystem));
        stored.update_labels_from(&newer);
        assert_eq!(
            stored.labels(),
            ContactLabels::Cached | ContactLabels::EndSystem
        );

        // explicitly advertising no labels clears them
        let mut newer = contact_with_path(&path, 4);
        newer.set_advertised_labels(Some(ContactLabels::empty()));
        stored.update_labels_from(&newer);
        assert_eq!(stored.labels(), ContactLabels::Cached);
    }

    #[test]
    fn inherit_labels_from_previous() {
        let _clock = crate::tests::TestClock::new();
        let path = [NodeId::from(1), NodeId::from(2)];
        let mut previous = contact_with_path(&path, 1);
        previous.add_local_labels(ContactLabels::Cached);
        previous.set_advertised_labels(Some(ContactLabels::Landmark));

        // without own label information everything is inherited
        let mut replacement = contact_with_path(&path, 2);
        replacement.inherit_labels_from(&previous);
        assert_eq!(
            replacement.labels(),
            ContactLabels::Cached | ContactLabels::Landmark
        );

        // own advertised labels win, local labels are combined
        let mut replacement = contact_with_path(&path, 2);
        replacement.set_advertised_labels(Some(ContactLabels::empty()));
        replacement.inherit_labels_from(&previous);
        assert_eq!(replacement.labels(), ContactLabels::Cached);
    }

    #[test]
    fn info_age_of_underlay_neighbor_is_zero() {
        let _clock = crate::tests::TestClock::new();
        let mut uln = contact_with_path(&[NodeId::from(1)], 1);
        uln.set_info_age(Age::from(10_000));
        assert_eq!(uln.info_age(), Age::from(0));
    }

    #[test]
    fn info_age_follows_path_validation() {
        let _clock = crate::tests::TestClock::new();
        let mut contact = contact_with_path(&[NodeId::from(1), NodeId::from(2)], 1);
        contact.set_info_age(Age::from(10_000));
        // without validation, the age of the contact information is used
        assert!(contact.info_age() >= Age::from(10_000));

        contact.path_mut().unwrap().set_valid();
        assert!(contact.info_age() < Age::from(1_000));
    }
}
