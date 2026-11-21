//! State of the PathId forwarding table kept by the routing tier.
//!
//! PathIds aggregate toward a sink: paths that merge in a node and use the same residual
//! path use the same PathId. The same entry may therefore be required for several reasons
//! at once: it is precomputed for the vicinity, used by local contacts and requested by
//! other nodes. [PathIdFwdState] tracks these reasons per entry, so that an entry is only
//! removed from the forwarding tables once nobody requires it anymore.
//!
//! Entries that are precomputed or used by local contacts are removed as soon as this reason
//! ceases to exist and no other is left. Entries requested by other nodes are _soft state_:
//! they have to be refreshed periodically (by PathSetupReq or ProbeReq messages traveling
//! along the path) and are removed by the
//! [garbage collection](PathIdFwdState::garbage_collect) otherwise.

use std::{
    collections::{
        HashMap,
        hash_map::Entry,
    },
    time::Duration,
};

use bitflags::bitflags;

use crate::domain::{
    PathId,
    Timestamp,
    UnderlayNeighborId,
    protocol_event::forwarding::{
        DecapsulationDestination,
        PathIdDecapsulationEntry,
        PathIdEntry,
        PathIdForwardingEntry,
        PathIdTableUpdate,
    },
};

bitflags! {
    /// Reasons why an entry of the PathId forwarding table is required.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct FwdEntryFlags: u8 {
        /// Precomputed for a path in the vicinity. Never expires.
        const PRECALC = 1;
        /// Used by at least one local contact. Never expires.
        const LOCAL = 1 << 1;
        /// Requested by another node (PathSetupReq or ProbeReq). Expires unless refreshed.
        const FOREIGN = 1 << 2;
    }
}

/// An entry of the PathId forwarding table together with its state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FwdStateEntry {
    entry: PathIdEntry,
    flags: FwdEntryFlags,
    /// Number of local contacts using this entry.
    local_refs: usize,
    last_refreshed: Timestamp,
    /// Whether the entry was ever requested or refreshed by a message.
    used: bool,
}

impl FwdStateEntry {
    /// The entry as installed in the forwarding tables.
    pub fn entry(&self) -> &PathIdEntry {
        &self.entry
    }

    /// Reasons why this entry is required.
    pub fn flags(&self) -> FwdEntryFlags {
        self.flags
    }

    /// Number of local contacts using this entry.
    pub fn local_refs(&self) -> usize {
        self.local_refs
    }

    /// Time of the last refresh.
    pub fn last_refreshed(&self) -> Timestamp {
        self.last_refreshed
    }

    /// Returns if the entry was ever requested or refreshed by a message.
    pub fn is_used(&self) -> bool {
        self.used
    }

    /// Returns if the entry is only kept as long as it is refreshed.
    pub fn is_soft_state(&self) -> bool {
        !self
            .flags
            .intersects(FwdEntryFlags::PRECALC | FwdEntryFlags::LOCAL)
    }

    /// Returns the underlay neighbor the entry forwards to, if any.
    pub fn next_hop(&self) -> Option<&UnderlayNeighborId> {
        match &self.entry {
            PathIdEntry::Forward(PathIdForwardingEntry { next_hop, .. }) => Some(next_hop),
            PathIdEntry::Decapsulate(PathIdDecapsulationEntry {
                next_hop: DecapsulationDestination::UnderlayNeighbor(next_hop),
                ..
            }) => Some(next_hop),
            PathIdEntry::Decapsulate(_) => None,
        }
    }
}

/// The PathId forwarding table of the routing tier, keyed by the incoming [PathId].
///
/// All methods changing the table return the [PathIdTableUpdate]s that have to be applied to
/// the forwarding tables. They are only returned if the entry visible to the forwarding
/// tables actually changes.
#[derive(Debug, Default)]
pub struct PathIdFwdState {
    entries: HashMap<PathId, FwdStateEntry>,
}

impl PathIdFwdState {
    /// Returns the entry with the given incoming [PathId].
    pub fn get(&self, in_path_id: &PathId) -> Option<&FwdStateEntry> {
        self.entries.get(in_path_id)
    }

    /// Returns if an entry with the given incoming [PathId] exists.
    pub fn contains(&self, in_path_id: &PathId) -> bool {
        self.entries.contains_key(in_path_id)
    }

    /// Iterates over all entries.
    pub fn iter(&self) -> impl Iterator<Item = (&PathId, &FwdStateEntry)> {
        self.entries.iter()
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns if there are no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Adds an entry precomputed for a path in the vicinity.
    pub fn add_precalc(&mut self, entry: PathIdEntry) -> Option<PathIdTableUpdate> {
        self.add(entry, FwdEntryFlags::PRECALC)
    }

    /// Adds a reference of a local contact to the entry.
    ///
    /// Every call has to be matched by a call of [PathIdFwdState::release_local].
    pub fn add_local(&mut self, entry: PathIdEntry) -> Option<PathIdTableUpdate> {
        self.add(entry, FwdEntryFlags::LOCAL)
    }

    /// Adds or refreshes an entry requested by another node.
    pub fn add_foreign(&mut self, entry: PathIdEntry) -> Option<PathIdTableUpdate> {
        self.add(entry, FwdEntryFlags::FOREIGN)
    }

    fn add(&mut self, entry: PathIdEntry, reason: FwdEntryFlags) -> Option<PathIdTableUpdate> {
        let now = Timestamp::now();
        match self.entries.entry(entry.in_path_id().clone()) {
            Entry::Occupied(mut occupied) => {
                let state = occupied.get_mut();
                state.set(reason);
                state.last_refreshed = now;
                if state.entry == entry {
                    None
                } else {
                    state.entry = entry.clone();
                    Some(PathIdTableUpdate::CreateOrUpdate(entry))
                }
            }
            Entry::Vacant(vacant) => {
                let mut state = FwdStateEntry {
                    entry: entry.clone(),
                    flags: FwdEntryFlags::empty(),
                    local_refs: 0,
                    last_refreshed: now,
                    used: false,
                };
                state.set(reason);
                vacant.insert(state);
                Some(PathIdTableUpdate::CreateOrUpdate(entry))
            }
        }
    }

    /// The entry is not precomputed anymore, as its path vanished from the vicinity.
    ///
    /// It is removed unless it is still required otherwise.
    pub fn del_precalc(&mut self, in_path_id: &PathId) -> Option<PathIdTableUpdate> {
        self.entries
            .get_mut(in_path_id)?
            .flags
            .remove(FwdEntryFlags::PRECALC);
        self.remove_if_unrequired(in_path_id)
    }

    /// Releases a reference of a local contact to the entry.
    ///
    /// It is removed unless it is still required otherwise.
    pub fn release_local(&mut self, in_path_id: &PathId) -> Option<PathIdTableUpdate> {
        let state = self.entries.get_mut(in_path_id)?;
        state.local_refs = state.local_refs.saturating_sub(1);
        if state.local_refs > 0 {
            return None;
        }
        state.flags.remove(FwdEntryFlags::LOCAL);
        self.remove_if_unrequired(in_path_id)
    }

    fn remove_if_unrequired(&mut self, in_path_id: &PathId) -> Option<PathIdTableUpdate> {
        if !self.entries.get(in_path_id)?.flags.is_empty() {
            return None;
        }
        self.entries.remove(in_path_id);
        Some(PathIdTableUpdate::Remove(in_path_id.clone()))
    }

    /// The entry is no longer requested by another node (PathTeardownReq).
    ///
    /// Other nodes may still require it and will set the flag again with their next
    /// refresh. Otherwise the entry expires.
    pub fn unset_foreign(&mut self, in_path_id: &PathId) {
        if let Some(state) = self.entries.get_mut(in_path_id) {
            state.flags.remove(FwdEntryFlags::FOREIGN);
        }
    }

    /// Refreshes the entry, as a message of a node using it traveled along the path.
    ///
    /// Returns if the entry exists.
    pub fn refresh(&mut self, in_path_id: &PathId) -> bool {
        match self.entries.get_mut(in_path_id) {
            Some(state) => {
                state.last_refreshed = Timestamp::now();
                state.used = true;
                true
            }
            None => false,
        }
    }

    /// Removes all entries forwarding to the given underlay neighbor, e.g. as it was lost.
    pub fn invalidate_via(&mut self, ulnid: &UnderlayNeighborId) -> Vec<PathIdTableUpdate> {
        let mut updates = Vec::new();
        self.entries.retain(|in_path_id, state| {
            if state.next_hop() != Some(ulnid) {
                return true;
            }
            updates.push(PathIdTableUpdate::Remove(in_path_id.clone()));
            false
        });
        updates
    }

    /// Removes all entries that are not required locally and haven't been refreshed for
    /// longer than `max_age`.
    pub fn garbage_collect(&mut self, max_age: Duration) -> Vec<PathIdTableUpdate> {
        let mut updates = Vec::new();
        self.entries.retain(|in_path_id, state| {
            if !state.is_soft_state() || state.last_refreshed.to_age_duration() <= max_age {
                return true;
            }
            updates.push(PathIdTableUpdate::Remove(in_path_id.clone()));
            false
        });
        updates
    }
}

impl FwdStateEntry {
    fn set(&mut self, reason: FwdEntryFlags) {
        if reason.contains(FwdEntryFlags::LOCAL) {
            self.local_refs += 1;
        }
        if reason.contains(FwdEntryFlags::FOREIGN) {
            self.used = true;
            // precomputed entries never expire, no need to track foreign interest
            if self.flags.contains(FwdEntryFlags::PRECALC) {
                return;
            }
        }
        self.flags.insert(reason);
        if reason.contains(FwdEntryFlags::PRECALC) {
            self.flags.remove(FwdEntryFlags::FOREIGN);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        domain::{
            ConnectionId,
            InterfaceId,
            NodeId,
        },
        tests::TestClock,
    };

    const MAX_AGE: Duration = Duration::from_secs(90);

    fn ulnid(interface: u32) -> UnderlayNeighborId {
        UnderlayNeighborId {
            interface_id: InterfaceId::try_from(interface).unwrap(),
            connection_id: ConnectionId::from(0),
        }
    }

    fn path_id(id: u8) -> PathId {
        PathId::from_shake256(&[NodeId::with_lsb(id)])
    }

    fn entry_via(in_id: u8, interface: u32) -> PathIdEntry {
        PathIdEntry::Forward(PathIdForwardingEntry {
            in_path_id: path_id(in_id),
            out_path_id: path_id(in_id + 100),
            next_hop: ulnid(interface),
        })
    }

    fn entry(in_id: u8) -> PathIdEntry {
        entry_via(in_id, 1)
    }

    fn is_create(update: &Option<PathIdTableUpdate>, expected: &PathIdEntry) -> bool {
        matches!(update, Some(PathIdTableUpdate::CreateOrUpdate(e)) if e == expected)
    }

    fn removed(updates: &[PathIdTableUpdate]) -> Vec<PathId> {
        updates
            .iter()
            .map(|update| match update {
                PathIdTableUpdate::Remove(id) => id.clone(),
                other => panic!("unexpected update {other}"),
            })
            .collect()
    }

    #[test]
    fn entry_is_installed_once() {
        let _clock = TestClock::new();
        let mut state = PathIdFwdState::default();

        assert!(is_create(&state.add_precalc(entry(1)), &entry(1)));
        // further reasons don't change the installed entry
        assert!(state.add_foreign(entry(1)).is_none());
        assert!(state.add_local(entry(1)).is_none());
        assert!(state.add_precalc(entry(1)).is_none());
        assert_eq!(state.len(), 1);
    }

    #[test]
    fn changed_entry_is_updated() {
        let _clock = TestClock::new();
        let mut state = PathIdFwdState::default();

        state.add_precalc(entry_via(1, 1));
        // e.g. the neighbor is now reachable via another interface
        assert!(is_create(
            &state.add_precalc(entry_via(1, 2)),
            &entry_via(1, 2)
        ));
    }

    #[test]
    fn foreign_entry_expires_without_refresh() {
        let mut clock = TestClock::new();
        let mut state = PathIdFwdState::default();
        state.add_foreign(entry(1));

        clock.advance(MAX_AGE);
        assert!(state.garbage_collect(MAX_AGE).is_empty());

        clock.advance(Duration::from_secs(1));
        assert_eq!(removed(&state.garbage_collect(MAX_AGE)), [path_id(1)]);
        assert!(state.is_empty());
    }

    #[test]
    fn refresh_keeps_foreign_entry_alive() {
        let mut clock = TestClock::new();
        let mut state = PathIdFwdState::default();
        state.add_foreign(entry(1));

        clock.advance(MAX_AGE);
        assert!(state.refresh(&path_id(1)));
        clock.advance(MAX_AGE);
        assert!(state.garbage_collect(MAX_AGE).is_empty());

        // a repeated request refreshes as well
        assert!(state.add_foreign(entry(1)).is_none());
        clock.advance(MAX_AGE);
        assert!(state.garbage_collect(MAX_AGE).is_empty());
    }

    #[test]
    fn refresh_of_unknown_entry() {
        let _clock = TestClock::new();
        let mut state = PathIdFwdState::default();
        assert!(!state.refresh(&path_id(1)));
    }

    #[test]
    fn precalc_and_local_entries_never_expire() {
        let mut clock = TestClock::new();
        let mut state = PathIdFwdState::default();
        state.add_precalc(entry(1));
        state.add_local(entry(2));

        clock.advance(10 * MAX_AGE);
        assert!(state.garbage_collect(MAX_AGE).is_empty());
        assert_eq!(state.len(), 2);
    }

    #[test]
    fn precalc_entry_ignores_foreign_interest() {
        let _clock = TestClock::new();
        let mut state = PathIdFwdState::default();
        state.add_precalc(entry(1));
        state.add_foreign(entry(1));

        let entry = state.get(&path_id(1)).unwrap();
        assert_eq!(entry.flags(), FwdEntryFlags::PRECALC);
        assert!(entry.is_used());
    }

    #[test]
    fn entry_precomputed_later_is_not_foreign_anymore() {
        let _clock = TestClock::new();
        let mut state = PathIdFwdState::default();
        state.add_foreign(entry(1));
        state.add_precalc(entry(1));

        assert_eq!(
            state.get(&path_id(1)).unwrap().flags(),
            FwdEntryFlags::PRECALC
        );
    }

    #[test]
    fn foreign_entry_survives_release_by_local_contact() {
        let mut clock = TestClock::new();
        let mut state = PathIdFwdState::default();
        state.add_foreign(entry(1));
        state.add_local(entry(1));

        // still requested by another node
        assert!(state.release_local(&path_id(1)).is_none());
        assert_eq!(
            state.get(&path_id(1)).unwrap().flags(),
            FwdEntryFlags::FOREIGN
        );

        clock.advance(MAX_AGE + Duration::from_secs(1));
        assert_eq!(removed(&state.garbage_collect(MAX_AGE)), [path_id(1)]);
    }

    #[test]
    fn entry_is_removed_with_its_precomputed_path() {
        let _clock = TestClock::new();
        let mut state = PathIdFwdState::default();
        state.add_precalc(entry(1));

        assert!(matches!(
            state.del_precalc(&path_id(1)),
            Some(PathIdTableUpdate::Remove(id)) if id == path_id(1)
        ));
        assert!(state.is_empty());
        assert!(state.del_precalc(&path_id(1)).is_none());
    }

    #[test]
    fn local_entry_survives_removal_of_precomputed_path() {
        let _clock = TestClock::new();
        let mut state = PathIdFwdState::default();
        state.add_precalc(entry(1));
        state.add_local(entry(1));

        assert!(state.del_precalc(&path_id(1)).is_none());
        assert_eq!(
            state.get(&path_id(1)).unwrap().flags(),
            FwdEntryFlags::LOCAL
        );
    }

    #[test]
    fn local_entry_is_kept_until_last_contact_releases_it() {
        let mut clock = TestClock::new();
        let mut state = PathIdFwdState::default();
        // two contacts share the same first path segment
        state.add_local(entry(1));
        state.add_local(entry(1));
        assert_eq!(state.get(&path_id(1)).unwrap().local_refs(), 2);

        assert!(state.release_local(&path_id(1)).is_none());
        clock.advance(10 * MAX_AGE);
        assert!(state.garbage_collect(MAX_AGE).is_empty());

        assert!(matches!(
            state.release_local(&path_id(1)),
            Some(PathIdTableUpdate::Remove(id)) if id == path_id(1)
        ));
        assert!(state.is_empty());
    }

    #[test]
    fn unset_foreign_lets_entry_expire() {
        let mut clock = TestClock::new();
        let mut state = PathIdFwdState::default();
        state.add_foreign(entry(1));
        state.unset_foreign(&path_id(1));
        assert!(state.get(&path_id(1)).unwrap().flags().is_empty());

        // a refresh of another node requiring the entry sets the flag again
        state.add_foreign(entry(1));
        assert_eq!(
            state.get(&path_id(1)).unwrap().flags(),
            FwdEntryFlags::FOREIGN
        );

        clock.advance(MAX_AGE + Duration::from_secs(1));
        assert_eq!(removed(&state.garbage_collect(MAX_AGE)), [path_id(1)]);
    }

    #[test]
    fn invalidate_via_removes_all_entries_of_neighbor() {
        let _clock = TestClock::new();
        let mut state = PathIdFwdState::default();
        state.add_precalc(entry_via(1, 1));
        state.add_foreign(entry_via(2, 1));
        state.add_local(entry_via(3, 2));
        state.add_precalc(PathIdEntry::Decapsulate(PathIdDecapsulationEntry {
            in_path_id: path_id(4),
            next_hop: DecapsulationDestination::Local,
        }));

        let mut removed = removed(&state.invalidate_via(&ulnid(1)));
        removed.sort_by_key(|id| id.as_ref().to_vec());
        let mut expected = [path_id(1), path_id(2)];
        expected.sort_by_key(|id| id.as_ref().to_vec());
        assert_eq!(removed, expected);
        assert_eq!(state.len(), 2);
    }
}
