//! Defines meta-data that is stored in the [VicinityGraph](super::VicinityGraph).

use std::time::Instant;

use crate::domain::SafeStateSeqNr;

/// An [Entry] of the [VicinityGraph](super::VicinityGraph).
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Entry {
    last_seen: Option<Instant>,
    synced_ssn: Option<SafeStateSeqNr>, // SSN up to which state has been synchronized (ULNDiscReq/Rsp or QueryRouteReq/Rsp exchange)
    observed_ssn: SafeStateSeqNr,
    /// A lower SSN than observed was seen: the node may have restarted.
    reset_suspected: bool,
}

impl Entry {
    /// Create a new [Entry].
    ///
    /// Since the entry hasn't been contacted initially [`last_seen`] and [`synced_ssn`] are [None].
    ///
    /// [`last_seen`]: fn@Entry::last_seen
    /// [`synced_ssn`]: fn@Entry::synced_ssn
    pub fn new(observed_ssn: SafeStateSeqNr) -> Self {
        // initially the vicinity node hasn't been seen
        // nor does it have stored vicinity information
        Self {
            last_seen: None,
            synced_ssn: None, // should only be updated by ULNDiscRsp
            observed_ssn,     // can be updated by any message
            reset_suspected: false,
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
    /// [synchronized state sequence number]: fn@Entry::synced_ssn
    pub fn update_synced_ssn(&mut self, synced_ssn: SafeStateSeqNr) {
        self.synced_ssn = Some(synced_ssn);
        // also update the observed_ssn in case it is smaller than synced_ssn
        if self.observed_ssn < synced_ssn {
            self.observed_ssn = synced_ssn;
        }
        // synchronized up to the greatest observed SSN: no restart happened
        if self.observed_ssn == synced_ssn {
            self.reset_suspected = false;
        }
    }

    /// Marks that a lower SSN than observed was seen, e.g., because the node restarted.
    ///
    /// The node has to be synchronized to get an authoritative answer.
    pub fn suspect_reset(&mut self) {
        self.reset_suspected = true;
    }

    /// Accepts the SSN of an authoritative answer of the node, also if it is lower,
    /// i.e., the node was reset.
    pub fn reset_ssn(&mut self, ssn: SafeStateSeqNr) {
        self.observed_ssn = ssn;
        self.synced_ssn = Some(ssn);
        self.reset_suspected = false;
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
        self.synced_ssn = None;
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

    /// State sequence number of the entry's vicinity present in the [VicinityGraph](super::VicinityGraph).
    ///
    /// The value can be [`None`] if the vicinity state of the node hasn't been acquired
    /// either because a synchronisation is ongoing or because of a node reset initialised.
    pub fn synced_ssn(&self) -> Option<&SafeStateSeqNr> {
        self.synced_ssn.as_ref()
    }

    /// Greatest observed state sequence number.
    ///
    /// The observed state sequence number can decrease on direct contact to a node.
    /// This is usually because of a reset initiated by the node caused by reaching
    /// the maximum state sequence number.
    pub fn observed_ssn(&self) -> &SafeStateSeqNr {
        &self.observed_ssn
    }

    /// Whether a lower SSN than observed was seen and the node has to be synchronized.
    pub fn is_reset_suspected(&self) -> bool {
        self.reset_suspected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssn(value: u32) -> SafeStateSeqNr {
        SafeStateSeqNr::try_from(value).unwrap()
    }

    #[test]
    fn reset_accepts_lower_ssn() {
        let mut entry = Entry::new(ssn(10));
        entry.update_synced_ssn(ssn(10));
        entry.suspect_reset();

        entry.reset_ssn(ssn(2));

        assert_eq!(*entry.observed_ssn(), ssn(2));
        assert_eq!(entry.synced_ssn(), Some(&ssn(2)));
        assert!(!entry.is_reset_suspected());
    }

    #[test]
    fn sync_up_to_observed_ssn_clears_suspicion() {
        let mut entry = Entry::new(ssn(10));
        entry.suspect_reset();

        entry.update_synced_ssn(ssn(9));
        assert!(
            entry.is_reset_suspected(),
            "not synchronized up to observed SSN"
        );

        entry.update_synced_ssn(ssn(10));
        assert!(!entry.is_reset_suspected());
    }
}
