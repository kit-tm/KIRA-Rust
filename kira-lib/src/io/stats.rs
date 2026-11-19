//! Counters of the sent and received [ProtocolMessages](kira_r2kad::domain::ProtocolMessage)
//! per [ProtocolMessageKind] for measuring the overhead of KIRA.
//!
//! The counters are updated by the [UdpSender](super::sender::udp_tokio::UdpSender) and
//! [UdpReceiver](super::receiver::udp_tokio::UdpReceiver) and can be read at any time with
//! [MessageStats::snapshot].

use std::{
    sync::atomic::{
        AtomicU64,
        Ordering,
    },
    time::{
        Duration,
        Instant,
    },
};

use kira_r2kad::domain::ProtocolMessageKind;

/// Size of the IPv6 (40 B) and UDP (8 B) headers of every datagram.
pub const IPV6_UDP_HEADER_BYTES: u64 = 48;

#[derive(Debug, Default)]
struct KindCounters {
    tx_msgs: AtomicU64,
    tx_bytes: AtomicU64,
    rx_msgs: AtomicU64,
    rx_bytes: AtomicU64,
}

/// Thread-safe counters of the sent and received messages and their (UDP payload) bytes
/// per [ProtocolMessageKind].
///
/// Each datagram is counted once, so a message sent to several interfaces counts once per
/// interface.
#[derive(Debug)]
pub struct MessageStats {
    start: Instant,
    /// indexed by the wire code of the [ProtocolMessageKind]
    per_kind: Box<[KindCounters; 256]>,
    rx_invalid_msgs: AtomicU64,
    rx_invalid_bytes: AtomicU64,
    tx_errors: AtomicU64,
}

impl Default for MessageStats {
    fn default() -> Self {
        Self::new()
    }
}

impl MessageStats {
    /// Creates zeroed counters, measuring the elapsed time from now.
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
            per_kind: Box::new(std::array::from_fn(|_| KindCounters::default())),
            rx_invalid_msgs: AtomicU64::new(0),
            rx_invalid_bytes: AtomicU64::new(0),
            tx_errors: AtomicU64::new(0),
        }
    }

    fn counters(&self, kind: ProtocolMessageKind) -> &KindCounters {
        &self.per_kind[usize::from(u8::from(kind))]
    }

    /// Counts a sent datagram with `payload_len` bytes of UDP payload.
    pub fn record_tx(&self, kind: ProtocolMessageKind, payload_len: usize) {
        let c = self.counters(kind);
        c.tx_msgs.fetch_add(1, Ordering::Relaxed);
        c.tx_bytes.fetch_add(payload_len as u64, Ordering::Relaxed);
    }

    /// Counts a received datagram with `payload_len` bytes of UDP payload.
    pub fn record_rx(&self, kind: ProtocolMessageKind, payload_len: usize) {
        let c = self.counters(kind);
        c.rx_msgs.fetch_add(1, Ordering::Relaxed);
        c.rx_bytes.fetch_add(payload_len as u64, Ordering::Relaxed);
    }

    /// Counts a received datagram that could not be deserialized.
    pub fn record_rx_invalid(&self, payload_len: usize) {
        self.rx_invalid_msgs.fetch_add(1, Ordering::Relaxed);
        self.rx_invalid_bytes
            .fetch_add(payload_len as u64, Ordering::Relaxed);
    }

    /// Counts a datagram whose sending failed.
    pub fn record_tx_error(&self) {
        self.tx_errors.fetch_add(1, Ordering::Relaxed);
    }

    /// Returns the current counter values of all kinds with any traffic.
    pub fn snapshot(&self) -> StatsSnapshot {
        let per_kind = self
            .per_kind
            .iter()
            .enumerate()
            .filter_map(|(code, c)| {
                let counts = KindSnapshot {
                    tx_msgs: c.tx_msgs.load(Ordering::Relaxed),
                    tx_bytes: c.tx_bytes.load(Ordering::Relaxed),
                    rx_msgs: c.rx_msgs.load(Ordering::Relaxed),
                    rx_bytes: c.rx_bytes.load(Ordering::Relaxed),
                };
                (counts != KindSnapshot::default())
                    .then(|| (ProtocolMessageKind::from(code as u8), counts))
            })
            .collect();

        StatsSnapshot {
            elapsed: self.start.elapsed(),
            per_kind,
            rx_invalid_msgs: self.rx_invalid_msgs.load(Ordering::Relaxed),
            rx_invalid_bytes: self.rx_invalid_bytes.load(Ordering::Relaxed),
            tx_errors: self.tx_errors.load(Ordering::Relaxed),
        }
    }
}

/// Counter values of one [ProtocolMessageKind] (or a sum of them).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct KindSnapshot {
    /// Sent datagrams
    pub tx_msgs: u64,
    /// UDP payload bytes of the sent datagrams
    pub tx_bytes: u64,
    /// Received datagrams
    pub rx_msgs: u64,
    /// UDP payload bytes of the received datagrams
    pub rx_bytes: u64,
}

impl KindSnapshot {
    /// Sent bytes including the IPv6 and UDP headers.
    pub fn tx_wire_bytes(&self) -> u64 {
        self.tx_bytes + self.tx_msgs * IPV6_UDP_HEADER_BYTES
    }

    /// Received bytes including the IPv6 and UDP headers.
    pub fn rx_wire_bytes(&self) -> u64 {
        self.rx_bytes + self.rx_msgs * IPV6_UDP_HEADER_BYTES
    }
}

impl std::ops::Add for KindSnapshot {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            tx_msgs: self.tx_msgs + other.tx_msgs,
            tx_bytes: self.tx_bytes + other.tx_bytes,
            rx_msgs: self.rx_msgs + other.rx_msgs,
            rx_bytes: self.rx_bytes + other.rx_bytes,
        }
    }
}

/// Values of [MessageStats] at one point in time.
#[derive(Debug, Clone)]
pub struct StatsSnapshot {
    /// Time since the counters were created
    pub elapsed: Duration,
    /// Counters of each kind with traffic, ordered by wire code
    pub per_kind: Vec<(ProtocolMessageKind, KindSnapshot)>,
    /// Received datagrams that could not be deserialized
    pub rx_invalid_msgs: u64,
    /// UDP payload bytes of [Self::rx_invalid_msgs]
    pub rx_invalid_bytes: u64,
    /// Datagrams whose sending failed
    pub tx_errors: u64,
}

impl StatsSnapshot {
    /// Sum over all kinds.
    pub fn total(&self) -> KindSnapshot {
        self.per_kind
            .iter()
            .fold(KindSnapshot::default(), |acc, (_, c)| acc + *c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_per_kind() {
        let stats = MessageStats::new();
        stats.record_tx(ProtocolMessageKind::ULNHello, 30);
        stats.record_tx(ProtocolMessageKind::ULNHello, 30);
        stats.record_rx(ProtocolMessageKind::FindNodeRsp, 500);
        stats.record_rx(ProtocolMessageKind::Other(0xff), 7);
        stats.record_rx_invalid(3);
        stats.record_tx_error();

        let snap = stats.snapshot();
        assert_eq!(snap.per_kind.len(), 3);
        assert_eq!(
            snap.per_kind[0],
            (
                ProtocolMessageKind::ULNHello,
                KindSnapshot {
                    tx_msgs: 2,
                    tx_bytes: 60,
                    rx_msgs: 0,
                    rx_bytes: 0
                }
            )
        );
        assert_eq!(snap.per_kind[0].1.tx_wire_bytes(), 60 + 2 * 48);
        assert_eq!(snap.per_kind[1].0, ProtocolMessageKind::FindNodeRsp);
        assert_eq!(snap.per_kind[2].0, ProtocolMessageKind::Other(0xff));
        assert_eq!(snap.per_kind[2].1.rx_wire_bytes(), 7 + 48);

        let total = snap.total();
        assert_eq!(total.tx_msgs, 2);
        assert_eq!(total.rx_msgs, 2);
        assert_eq!(total.rx_bytes, 507);
        assert_eq!(snap.rx_invalid_msgs, 1);
        assert_eq!(snap.rx_invalid_bytes, 3);
        assert_eq!(snap.tx_errors, 1);
    }

    #[test]
    fn empty_snapshot() {
        let snap = MessageStats::new().snapshot();
        assert!(snap.per_kind.is_empty());
        assert_eq!(snap.total(), KindSnapshot::default());
    }
}
