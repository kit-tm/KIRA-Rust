//! Loading and managing the eBPF program of the SRv6 forwarding.
//!
//! The program (`kira-forwarding-ebpf`) is built by the build script of this crate and
//! embedded into the binary. It is attached to the ingress of the underlay interfaces and
//! handles the packets whose destination is a PathIP according to the [PathTable].
//! Packets with an unknown PathIP are dropped and reported through [DropEvents].

use std::{
    collections::HashSet,
    net::Ipv6Addr,
};

use aya::{
    Ebpf,
    EbpfError,
    EbpfLoader,
    maps::{
        HashMap,
        MapData,
        MapError,
        RingBuf,
    },
    programs::{
        ProgramError,
        SchedClassifier,
        TcAttachType,
        tc,
    },
};
use derive_more::derive::{
    Display,
    Error,
    From,
};
use kira_forwarding_common::{
    DROP_EVENTS,
    DroppedPacketEvent,
    INGRESS_PROGRAM,
    PATH_IP_PREFIX,
    PATH_TABLE,
    PathAction,
    PathIp,
};
use kira_r2kad::domain::protocol_event::DroppedPacket;
use tokio::{
    io::unix::AsyncFd,
    sync::mpsc::{
        Sender,
        error::TrySendError,
    },
};

use crate::domain::{
    InterfaceId,
    IpPrefixes,
};

/// The eBPF object built from `kira-forwarding-ebpf`.
static PROGRAM: &[u8] =
    aya::include_bytes_aligned!(concat!(env!("OUT_DIR"), "/kira-forwarding-ebpf"));

/// Error codes of system calls for things that don't exist.
const ENOENT: i32 = 2;

#[derive(Debug, Display, Error, From)]
/// Error of loading or managing the eBPF program.
pub enum ForwardingEbpfError {
    /// Loading the eBPF object failed.
    #[display("Loading the eBPF object failed: {_0}")]
    Load(EbpfError),
    /// Loading or attaching the eBPF program failed.
    #[display("eBPF program error: {_0}")]
    Program(ProgramError),
    /// Accessing an eBPF map failed.
    #[display("eBPF map error: {_0}")]
    Map(MapError),
    /// The eBPF object doesn't contain the expected program or map.
    #[display("eBPF object lacks {_0}")]
    #[from(ignore)]
    Missing(#[error(not(source))] &'static str),
}

/// Result used by this module.
pub type Result<T> = std::result::Result<T, ForwardingEbpfError>;

/// Loads the eBPF program for PathIPs with the prefix given by `ip_prefixes`.
///
/// Returns the program, which has to be [attached](IngressProgram::attach) to the underlay
/// interfaces, the table of the PathIPs it handles and the reports of the packets it dropped.
/// The program is detached from all interfaces once the [IngressProgram] is dropped.
pub fn load(ip_prefixes: &IpPrefixes) -> Result<(IngressProgram, PathTable, DropEvents)> {
    let path_ip_prefix = ip_prefixes.path_ip_prefix().bits().to_be_bytes();
    let mut ebpf = EbpfLoader::new()
        .override_global(PATH_IP_PREFIX, &path_ip_prefix, true)
        .load(PROGRAM)?;

    let program: &mut SchedClassifier = ebpf
        .program_mut(INGRESS_PROGRAM)
        .ok_or(ForwardingEbpfError::Missing(INGRESS_PROGRAM))?
        .try_into()?;
    program.load()?;

    let map = ebpf
        .take_map(PATH_TABLE)
        .ok_or(ForwardingEbpfError::Missing(PATH_TABLE))?;
    let map = HashMap::try_from(map)?;

    let events = ebpf
        .take_map(DROP_EVENTS)
        .ok_or(ForwardingEbpfError::Missing(DROP_EVENTS))?;
    let events = RingBuf::try_from(events)?;

    Ok((
        IngressProgram {
            ebpf,
            attached: HashSet::new(),
        },
        PathTable { map },
        DropEvents { events },
    ))
}

/// The eBPF program handling packets with PathIPs on ingress of the underlay interfaces.
#[derive(Debug)]
pub struct IngressProgram {
    ebpf: Ebpf,
    attached: HashSet<InterfaceId>,
}

impl IngressProgram {
    /// Attaches the program to the ingress of the interface.
    ///
    /// Attaching to an interface the program is already attached to has no effect.
    pub fn attach(&mut self, interface: InterfaceId) -> Result<()> {
        if self.attached.contains(&interface) {
            return Ok(());
        }
        let if_index = u32::from(interface);

        // Kernels without TCX (< 6.6) attach through netlink, which needs the clsact qdisc
        // and leaves the program attached if a previous instance was not shut down cleanly.
        let _ = tc::qdisc_add_clsact(if_index);
        let _ = tc::qdisc_detach_program(if_index, TcAttachType::Ingress.into(), INGRESS_PROGRAM);

        let program: &mut SchedClassifier = self
            .ebpf
            .program_mut(INGRESS_PROGRAM)
            .ok_or(ForwardingEbpfError::Missing(INGRESS_PROGRAM))?
            .try_into()?;
        program.attach(if_index, TcAttachType::Ingress)?;
        self.attached.insert(interface);

        log::debug!(target: "srv6_fwd_table::ebpf", "attached eBPF program to interface {interface}");
        Ok(())
    }
}

/// The table of the incoming PathIPs handled by the [IngressProgram].
#[derive(Debug)]
pub struct PathTable {
    map: HashMap<MapData, PathIp, PathAction>,
}

impl PathTable {
    /// Packets destined to `in_path_ip` get `out_path_ip` as destination instead
    /// and are forwarded by the network stack.
    pub fn set_swap(&mut self, in_path_ip: Ipv6Addr, out_path_ip: Ipv6Addr) -> Result<()> {
        self.set(in_path_ip, PathAction::swap(out_path_ip.octets()))
    }

    /// The path segment of packets destined to `in_path_ip` ends at this node.
    ///
    /// They continue on their next path segment or are decapsulated.
    pub fn set_end(&mut self, in_path_ip: Ipv6Addr) -> Result<()> {
        self.set(in_path_ip, PathAction::end())
    }

    fn set(&mut self, in_path_ip: Ipv6Addr, action: PathAction) -> Result<()> {
        self.map.insert(&in_path_ip.octets(), &action, 0)?;
        log::debug!(target: "srv6_fwd_table::ebpf", "path table entry set: {in_path_ip} => {action:?}");
        Ok(())
    }

    /// Packets destined to `in_path_ip` are not handled anymore.
    ///
    /// Doesn't fail if there is no entry for `in_path_ip`.
    pub fn remove(&mut self, in_path_ip: Ipv6Addr) -> Result<()> {
        match self.map.remove(&in_path_ip.octets()) {
            Ok(()) => {}
            Err(MapError::SyscallError(error)) if error.io_error.raw_os_error() == Some(ENOENT) => {
            }
            Err(error) => return Err(error.into()),
        }
        log::debug!(target: "srv6_fwd_table::ebpf", "path table entry removed: {in_path_ip}");
        Ok(())
    }
}

/// Reports of the packets the [IngressProgram] dropped because of an unknown PathIP.
pub struct DropEvents {
    events: RingBuf<MapData>,
}

impl std::fmt::Debug for DropEvents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DropEvents").finish_non_exhaustive()
    }
}

impl DropEvents {
    /// Receives the reports of dropped packets and sends them to `dropped` until the channel
    /// is closed.
    ///
    /// Reports are discarded while the channel is full: the routing tier only needs to learn
    /// about obsolete paths, not about every packet (the eBPF program also limits the reports
    /// per source and PathIP).
    pub async fn listen(
        self,
        ip_prefixes: IpPrefixes,
        dropped: Sender<DroppedPacket>,
    ) -> std::io::Result<()> {
        // The ring buffer owns its file descriptor and is owned by the AsyncFd, so the
        // descriptor can't be closed while registered (the I/O safety issue of `new`).
        // `AsyncFd::register` would need unsafe code, which this crate forbids.
        #[allow(deprecated)]
        let mut events = AsyncFd::new(self.events)?;
        loop {
            let mut guard = tokio::select! {
                guard = events.readable_mut() => guard?,
                () = dropped.closed() => return Ok(()),
            };
            let ring = guard.get_inner_mut();
            while let Some(item) = ring.next() {
                let Some(packet) = parse_event(&item, &ip_prefixes) else {
                    continue;
                };
                log::debug!(target: "srv6_fwd_table::ebpf", "dropped packet with unknown PathIP {}", packet.path_ip);
                match dropped.try_send(packet) {
                    Ok(()) | Err(TrySendError::Full(_)) => {}
                    Err(TrySendError::Closed(_)) => return Ok(()),
                }
            }
            guard.clear_ready();
        }
    }
}

/// Converts a [DroppedPacketEvent] of the eBPF program.
///
/// Returns `None` if the event is malformed or the packet wasn't encapsulated by a KIRA node.
fn parse_event(bytes: &[u8], ip_prefixes: &IpPrefixes) -> Option<DroppedPacket> {
    const ADDR: usize = 16;
    let field = |index: usize| -> Option<[u8; ADDR]> {
        bytes.get(index * ADDR..(index + 1) * ADDR)?.try_into().ok()
    };
    // the fields of DroppedPacketEvent in their order (repr(C), byte arrays only)
    let event = DroppedPacketEvent {
        source: field(0)?,
        path_ip: field(1)?,
        destination: field(2)?,
    };
    let encapsulator = ip_prefixes.node_id(&Ipv6Addr::from(event.source))?;
    let path_ip = Ipv6Addr::from(event.path_ip);
    if !ip_prefixes.is_path_ip(&path_ip) {
        return None;
    }
    let destination = ip_prefixes.node_id(&Ipv6Addr::from(event.destination));
    Some(DroppedPacket {
        encapsulator,
        path_ip,
        destination,
    })
}

#[cfg(test)]
mod tests {
    use kira_r2kad::domain::{
        NodeId,
        PathId,
    };

    use super::*;

    #[test]
    fn parses_dropped_packet_events() {
        let ip_prefixes = IpPrefixes::default();
        let encapsulator = NodeId::with_msb(3);
        let destination = NodeId::with_msb(5);
        let path_ip = ip_prefixes.path_ip(&PathId::from_shake256(&[NodeId::with_msb(4)]));

        let mut bytes = Vec::new();
        bytes.extend(ip_prefixes.node_ip(&encapsulator).octets());
        bytes.extend(path_ip.octets());
        bytes.extend(ip_prefixes.node_ip(&destination).octets());
        assert_eq!(bytes.len(), size_of::<DroppedPacketEvent>());

        assert_eq!(
            parse_event(&bytes, &ip_prefixes),
            Some(DroppedPacket {
                encapsulator,
                path_ip,
                destination: Some(destination),
            })
        );

        // unknown inner destination
        bytes[32..].fill(0);
        assert_eq!(
            parse_event(&bytes, &ip_prefixes).map(|packet| packet.destination),
            Some(None)
        );
        // truncated
        assert_eq!(parse_event(&bytes[..40], &ip_prefixes), None);
        // no PathIP
        bytes[16..32].copy_from_slice(&ip_prefixes.node_ip(&destination).octets());
        assert_eq!(parse_event(&bytes, &ip_prefixes), None);
    }
}
