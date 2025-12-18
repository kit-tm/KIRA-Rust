//! Defines meta-data that is stored in the [VicinityGraph](super::VicinityGraph).

use std::time::Instant;

use crate::domain::SafeStateSeqNr;

/// An [Entry] of the [VicinityGraph](super::VicinityGraph).
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Entry {
    last_seen: Option<Instant>,
    synched_ssn: Option<SafeStateSeqNr>, // SSN up to which state has been synchronized (ULNDiscReq/Rsp or QueryRouteReq/Rsp exchange)
    observed_ssn: SafeStateSeqNr,
}

impl Entry {
    /// Create a new [Entry].
    ///
    /// Since the entry hasn't been contacted initially [`last_seen`] and [`synched_ssn`] are [None].
    ///
    /// [`last_seen`]: fn@Entry::last_seen
    /// [`synched_ssn`]: fn@Entry::synched_ssn
    pub fn new(observed_ssn: SafeStateSeqNr) -> Self {
        // initially the vicinity node hasn't been seen
        // nor does it have stored vicinity information
        Self {
            last_seen: None,
            synched_ssn: None, // should only be updated by ULNDisRsp or QueryRouteRsp
            observed_ssn,      // can be updated by any message
        }
    }

    /// Update the latest [observed state sequence number].
    ///
    /// The state sequence number can be decreased on resets only.
    ///
    /// [observed state sequence number]: fn@Entry::observed_ssn
    pub fn update_observed_ssn(&mut self, observed_ssn: SafeStateSeqNr) {
        if observed_ssn > self.observed_ssn {
            self.observed_ssn = observed_ssn;
        }
    }

    /// Update the [synchronized state sequence number].
    ///
    /// [synchronized state sequence number]: fn@Entry::synched_ssn
    pub fn update_synched_ssn(&mut self, synched_ssn: SafeStateSeqNr) {
        self.synched_ssn = Some(synched_ssn);
        // also update the observed_ssn in case it is smaller than synched_ssn
        if self.observed_ssn < synched_ssn {
            self.observed_ssn = synched_ssn;
        }
    }

    /// Update the last time the node was successfully contacted *directly*
    /// (a response received to a request).
    ///
    /// Last seen has to be updated monotonically nondecreasing otherwise
    /// the method will panic.
    pub fn update_last_seen(&mut self, now: Instant) {
        assert!(
            self.last_seen.is_none_or(|l| l <= now),
            "last seen can't move backwards in time"
        );

        self.last_seen = Some(now);
    }

    /// Forgets the vicinity state sequence number.
    pub fn forget_vicinity(&mut self) {
        self.synched_ssn = None;
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

    /// synchronized state sequence number present in the [VicinityGraph](super::VicinityGraph).
    ///
    /// The value can be [`None`] if the vicinity state of the node hasn't been acquired
    /// either because a synchronisation is ongoing or because of a node reset initialised.
    pub fn synched_ssn(&self) -> Option<&SafeStateSeqNr> {
        self.synched_ssn.as_ref()
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
