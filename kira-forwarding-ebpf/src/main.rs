//! eBPF program of the SRv6-based KIRA fast forwarding.
//!
//! It is attached to the ingress of all underlay interfaces of a KIRA node (TC classifier)
//! and handles the packets whose outer IPv6 destination address is a PathIP:
//!
//! - **Swap**: the destination address is replaced by the PathIP of the remaining path
//!   segment. This in-place rewrite by intermediate nodes is the part of KIRA's forwarding
//!   that regular SRv6 endpoints don't provide.
//! - **End**: the path segment ends at this node. If the packet has a Segment Routing Header
//!   with segments left, it continues with the next segment (whose PathIP is looked up
//!   again). Otherwise the outer IPv6 header (and the Segment Routing Header) is removed.
//!
//! Everything else is left to the network stack: forwarding to the next hop of a PathIP,
//! local delivery or forwarding of decapsulated packets and the encapsulation of packets
//! (`seg6` routes).
//!
//! Packets with unknown PathIPs, e.g., as the link to the next hop failed, are dropped and
//! reported to the routing tier (rate limited), which notifies the node that used the
//! obsolete path.

#![cfg_attr(target_arch = "bpf", no_std)]
#![cfg_attr(target_arch = "bpf", no_main)]

#[cfg(target_arch = "bpf")]
mod ingress;

#[cfg(not(target_arch = "bpf"))]
fn main() {
    panic!("kira-forwarding-ebpf has to be built for a BPF target");
}
