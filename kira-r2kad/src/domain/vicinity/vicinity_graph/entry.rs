//! Defines meta-data that is stored in the [VicinityGraph](super::VicinityGraph).

use std::{cmp, time::Instant};

use crate::domain::SafeStateSeqNr;

/// An [Entry] of the [VicinityGraph](super::VicinityGraph).
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Entry {
    last_seen: Option<Instant>,
    vicinity_ssn: Option<SafeStateSeqNr>,
    observed_ssn: SafeStateSeqNr,
}

impl Entry {
    /// Create a new [Entry].
    ///
    /// Since the entry hasn't been contacted initially [`last_seen`] and [`vicinity_ssn`] are [None].
    ///
    /// [`last_seen`]: fn@Entry::last_seen
    /// [`vicinity_ssn`]: fn@Entry::vicinity_ssn
    pub fn new(observed_ssn: SafeStateSeqNr) -> Self {
        // initially the vicinity node hasn't been seen
        // nor does it have stored vicinity information
        Self {
            last_seen: None,
            vicinity_ssn: None,
            observed_ssn,
        }
    }

    /// Update the latest [observed state sequence number].
    ///
    /// The state sequence number can be decreased on resets.
    ///
    /// [observed state sequence number]: fn@Entry::observed_ssn
    pub fn update_observed_ssn(&mut self, observed_ssn: SafeStateSeqNr) {
        self.observed_ssn = observed_ssn;
    }

    /// Update the [state sequence number of the vicinity].
    ///
    /// [state sequence number of the vicinity]: fn@Entry::vicinity_ssn
    pub fn update_vicinity_ssn(&mut self, vicinity_ssn: SafeStateSeqNr) {
        self.observed_ssn = cmp::max(self.observed_ssn, vicinity_ssn);
        self.vicinity_ssn = Some(vicinity_ssn);
    }

    /// Update the last time the node was successfully contacted *directly*
    /// (a response received to a request).
    ///
    /// Last seen has to be updated monotonically nondecreasing otherwise
    /// the method will panic.
    pub fn update_last_seen(&mut self, now: Instant) {
        assert!(
            self.last_seen.is_none_or(|l| l < now),
            "last seen can't move backwards in time"
        );

        self.last_seen = Some(now);
    }

    /// Forgets the vicinity state sequence number.
    pub fn forget_vicinity(&mut self) {
        self.vicinity_ssn = None;
    }
}

// Getters
impl Entry {
    /// Last time the node was successfully contacted *directly*
    /// (a response received to a request).
    ///
    /// If the node was never contacted directly the value is [`None`].
    pub fn last_seen(&self) -> Option<Instant> {
        self.last_seen
    }

    /// State sequence number of the vicinity present in the [VicinityGraph](super::VicinityGraph).
    ///
    /// The value can be [`None`] if the vicinity state of the node hasn't been acquired
    /// either because a synchronisation is ongoing or because of a node reset initialised.
    pub fn vicinity_ssn(&self) -> Option<&SafeStateSeqNr> {
        self.vicinity_ssn.as_ref()
    }

    /// Greatest observed state sequence number.
    ///
    /// The observed state sequence number can decrease on direct contact to a node.
    /// This is usually because of a reset initiated by the node caused by reaching
    /// the maximum state sequence number.
    pub fn observed_ssn(&self) -> &SafeStateSeqNr {
        &self.observed_ssn
    }
}
