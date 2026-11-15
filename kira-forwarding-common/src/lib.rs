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
