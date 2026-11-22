//! Types shared by the KIRA eBPF forwarding program (`kira-forwarding-ebpf`)
//! and the user space managing its maps (`kira-forwarding`).

#![no_std]

/// Name of the eBPF program handling packets on ingress of the underlay interfaces.
pub const INGRESS_PROGRAM: &str = "kira_ingress";

/// Name of the map from incoming PathIPs to [PathActions](PathAction).
pub const PATH_TABLE: &str = "PATH_TABLE";

/// Name of the global variable holding the 16 bit prefix of all PathIPs in network byte order.
pub const PATH_IP_PREFIX: &str = "PATH_IP_PREFIX";

/// Maximum number of entries of the [PATH_TABLE].
pub const PATH_TABLE_SIZE: u32 = 1 << 16;

/// Name of the ring buffer of [DroppedPacketEvents](DroppedPacketEvent).
pub const DROP_EVENTS: &str = "DROP_EVENTS";

/// Size of the [DROP_EVENTS] ring buffer in bytes (a power of two and multiple of the page
/// size).
pub const DROP_EVENTS_SIZE: u32 = 1 << 16;

/// Maximum number of (source, PathIP) pairs whose last report is remembered for the rate
/// limiting of [DroppedPacketEvents](DroppedPacketEvent).
pub const DROP_REPORTS_SIZE: u32 = 1 << 12;

/// Minimum interval in nanoseconds between two [DroppedPacketEvents](DroppedPacketEvent) for
/// packets of the same source and PathIP.
pub const DROP_REPORT_INTERVAL_NS: u64 = 100_000_000;

/// An IPv6 address derived from a PathId, the key of the [PATH_TABLE].
pub type PathIp = [u8; 16];

/// What to do with a packet whose outer destination address is a known PathIP.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PathAction {
    /// One of [PathAction::SWAP] and [PathAction::END].
    pub kind: u32,
    /// The PathIP replacing the destination address for [PathAction::SWAP].
    pub out_path_ip: PathIp,
}

impl PathAction {
    /// The node is the last one of the path segment.
    ///
    /// The packet continues on its next path segment, if its Segment Routing Header
    /// contains one. Otherwise the outer header is removed and the inner packet is
    /// handled by the network stack.
    pub const END: u32 = 2;
    /// The destination address is replaced by the PathIP of the remaining path
    /// and the packet is forwarded by the network stack.
    pub const SWAP: u32 = 1;

    /// Swap the PathIP with the given one.
    pub const fn swap(out_path_ip: PathIp) -> Self {
        Self {
            kind: Self::SWAP,
            out_path_ip,
        }
    }

    /// The path segment ends here.
    pub const fn end() -> Self {
        Self {
            kind: Self::END,
            out_path_ip: [0; 16],
        }
    }
}

// SAFETY: PathAction is `repr(C)`, has no padding (4 + 16 bytes, alignment 4)
// and every bit pattern is valid.
#[cfg(feature = "user")]
unsafe impl aya::Pod for PathAction {}

/// A packet dropped because its PathIP is unknown, reported by the eBPF program to the
/// routing tier, which notifies the node that used the obsolete path.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DroppedPacketEvent {
    /// Outer source address: the node that encapsulated the packet.
    pub source: [u8; 16],
    /// The unknown PathIP.
    pub path_ip: PathIp,
    /// Destination of the encapsulated packet, all zeros if unknown.
    pub destination: [u8; 16],
}

/// Key of the rate limiting of [DroppedPacketEvents](DroppedPacketEvent).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DropReportKey {
    /// See [DroppedPacketEvent::source].
    pub source: [u8; 16],
    /// See [DroppedPacketEvent::path_ip].
    pub path_ip: PathIp,
}

// SAFETY: DroppedPacketEvent is `repr(C)`, consists of byte arrays only (no padding)
// and every bit pattern is valid.
#[cfg(feature = "user")]
unsafe impl aya::Pod for DroppedPacketEvent {}
