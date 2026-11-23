use aya_ebpf::{
    bindings::{
        TC_ACT_OK,
        TC_ACT_SHOT,
        bpf_adj_room_mode::BPF_ADJ_ROOM_MAC,
    },
    helpers::bpf_ktime_get_ns,
    macros::{
        classifier,
        map,
    },
    maps::{
        HashMap,
        LruHashMap,
        RingBuf,
    },
    programs::TcContext,
};
use kira_forwarding_common::{
    DROP_EVENTS_SIZE,
    DROP_REPORT_INTERVAL_NS,
    DROP_REPORTS_SIZE,
    DropReportKey,
    DroppedPacketEvent,
    PATH_TABLE_SIZE,
    PathAction,
    PathIp,
};

/// Incoming PathIPs and what to do with their packets.
#[map]
static PATH_TABLE: HashMap<PathIp, PathAction> = HashMap::with_max_entries(PATH_TABLE_SIZE, 0);

/// Packets dropped because of an unknown PathIP, read by the routing tier.
#[map]
static DROP_EVENTS: RingBuf = RingBuf::with_byte_size(DROP_EVENTS_SIZE, 0);

/// Time of the last report per source and PathIP, for rate limiting the reports.
#[map]
static DROP_REPORTS: LruHashMap<DropReportKey, u64> =
    LruHashMap::with_max_entries(DROP_REPORTS_SIZE, 0);

/// The 16 bit prefix of all PathIPs in network byte order, set when the program is loaded.
#[unsafe(no_mangle)]
static PATH_IP_PREFIX: [u8; 2] = [0xfc, 0xaa];

const ETH_HDR_LEN: usize = 14;
/// Offset of the EtherType in the Ethernet header.
const ETH_TYPE_OFFSET: usize = 12;
const ETH_P_IPV6: [u8; 2] = [0x86, 0xdd];

const IPV6_HDR_LEN: usize = 40;
/// Offset of the Next Header field in the IPv6 header.
const IPV6_NEXT_HDR_OFFSET: usize = 6;
/// Offset of the Hop Limit field in the IPv6 header.
const IPV6_HOP_LIMIT_OFFSET: usize = 7;
/// Offset of the source address in the IPv6 header.
const IPV6_SRC_OFFSET: usize = 8;
/// Offset of the destination address in the IPv6 header.
const IPV6_DST_OFFSET: usize = 24;

const IPPROTO_IPV6: u8 = 41;
const IPPROTO_ROUTING: u8 = 43;

/// Routing type of the Segment Routing Header (RFC 8754).
const SRH_ROUTING_TYPE: u8 = 4;
/// Length of the fixed part of the Segment Routing Header.
const SRH_FIXED_LEN: usize = 8;
/// Offset of the Segments Left field in the Segment Routing Header.
const SRH_SEGMENTS_LEFT_OFFSET: usize = 3;
/// Largest index of a segment the program handles.
///
/// KIRA uses a Reduced Segment Routing Header with a single segment (the second path
/// segment); with Fast Next Hop Detour there may be one more.
const MAX_SEGMENT_INDEX: u8 = 3;

/// Number of PathIPs handled at one node: the end of a path segment may be followed by the
/// start of the next one.
const MAX_LOOKUPS: usize = 3;

/// First bytes of a Segment Routing Header.
#[repr(C)]
#[derive(Clone, Copy)]
struct SrhFixed {
    next_header: u8,
    /// Length of the header in units of 8 bytes without the first 8 bytes.
    hdr_ext_len: u8,
    routing_type: u8,
    segments_left: u8,
    last_entry: u8,
    flags: u8,
    tag: [u8; 2],
}

#[classifier]
pub fn kira_ingress(ctx: TcContext) -> i32 {
    // Packets that can't be handled are passed on to the network stack unchanged.
    try_kira_ingress(&ctx).unwrap_or(TC_ACT_OK as i32)
}

#[inline(always)]
fn try_kira_ingress(ctx: &TcContext) -> Result<i32, i64> {
    let ether_type: [u8; 2] = ctx.load(ETH_TYPE_OFFSET)?;
    if ether_type != ETH_P_IPV6 {
        return Ok(TC_ACT_OK as i32);
    }

    // SAFETY: plain bytes that are only read; the loader may replace them before loading.
    let prefix = unsafe { core::ptr::read_volatile(&raw const PATH_IP_PREFIX) };

    for _ in 0..MAX_LOOKUPS {
        let dst: PathIp = ctx.load(ETH_HDR_LEN + IPV6_DST_OFFSET)?;
        if dst[0] != prefix[0] || dst[1] != prefix[1] {
            return Ok(TC_ACT_OK as i32);
        }

        // SAFETY: the value is copied before the map can be changed by this program,
        // which never writes to the map.
        let Some(action) = (unsafe { PATH_TABLE.get(&dst) }).copied() else {
            // unknown PathIP, e.g., the link to the next hop failed: the routing tier
            // notifies the node that used the obsolete path
            report_dropped(ctx, dst);
            return Ok(TC_ACT_SHOT as i32);
        };

        if action.kind == PathAction::SWAP {
            ctx.store(ETH_HDR_LEN + IPV6_DST_OFFSET, &action.out_path_ip, 0)?;
            return Ok(TC_ACT_OK as i32);
        }
        if action.kind != PathAction::END {
            return Ok(TC_ACT_OK as i32);
        }

        // End of the path segment
        let next_header: u8 = ctx.load(ETH_HDR_LEN + IPV6_NEXT_HDR_OFFSET)?;
        if next_header == IPPROTO_IPV6 {
            // single path segment without Segment Routing Header
            return decapsulate(ctx, IPV6_HDR_LEN);
        }
        if next_header != IPPROTO_ROUTING {
            return Ok(TC_ACT_OK as i32);
        }

        let srh_offset = ETH_HDR_LEN + IPV6_HDR_LEN;
        let srh: SrhFixed = ctx.load(srh_offset)?;
        if srh.routing_type != SRH_ROUTING_TYPE {
            return Ok(TC_ACT_OK as i32);
        }

        if srh.segments_left == 0 {
            // last path segment
            if srh.next_header != IPPROTO_IPV6 {
                return Ok(TC_ACT_OK as i32);
            }
            let srh_len = SRH_FIXED_LEN + (srh.hdr_ext_len as usize) * 8;
            return decapsulate(ctx, IPV6_HDR_LEN + srh_len);
        }

        // Continue with the next path segment
        let segments_left = srh.segments_left - 1;
        if segments_left > srh.last_entry || segments_left > MAX_SEGMENT_INDEX {
            return Ok(TC_ACT_OK as i32);
        }
        let segment_offset = srh_offset + SRH_FIXED_LEN + (segments_left as usize) * 16;
        let segment: PathIp = ctx.load(segment_offset)?;

        ctx.store(srh_offset + SRH_SEGMENTS_LEFT_OFFSET, &segments_left, 0)?;
        ctx.store(ETH_HDR_LEN + IPV6_DST_OFFSET, &segment, 0)?;
        // this node may be the first one of the next path segment
    }

    Ok(TC_ACT_OK as i32)
}

/// Reports a packet with the unknown PathIP `path_ip` to the routing tier, at most once per
/// [DROP_REPORT_INTERVAL_NS] for the same source and PathIP.
#[inline(always)]
fn report_dropped(ctx: &TcContext, path_ip: PathIp) {
    let Ok(source) = ctx.load::<[u8; 16]>(ETH_HDR_LEN + IPV6_SRC_OFFSET) else {
        return;
    };
    let key = DropReportKey { source, path_ip };
    // SAFETY: bpf_ktime_get_ns has no preconditions.
    let now = unsafe { bpf_ktime_get_ns() };
    // SAFETY: the value is copied right away.
    if let Some(last) = unsafe { DROP_REPORTS.get(&key) }.copied()
        && now.wrapping_sub(last) < DROP_REPORT_INTERVAL_NS
    {
        return;
    }
    let _ = DROP_REPORTS.insert(&key, &now, 0);

    let event = DroppedPacketEvent {
        source,
        path_ip,
        destination: inner_destination(ctx).unwrap_or([0; 16]),
    };
    // a full ring buffer loses the report
    let _ = DROP_EVENTS.output(&event, 0);
}

/// Destination address of the encapsulated IPv6 packet, behind the outer IPv6 header and an
/// optional Segment Routing Header.
#[inline(always)]
fn inner_destination(ctx: &TcContext) -> Option<[u8; 16]> {
    let next_header: u8 = ctx.load(ETH_HDR_LEN + IPV6_NEXT_HDR_OFFSET).ok()?;
    let inner_offset = if next_header == IPPROTO_IPV6 {
        ETH_HDR_LEN + IPV6_HDR_LEN
    } else if next_header == IPPROTO_ROUTING {
        let srh: SrhFixed = ctx.load(ETH_HDR_LEN + IPV6_HDR_LEN).ok()?;
        if srh.next_header != IPPROTO_IPV6 {
            return None;
        }
        ETH_HDR_LEN + IPV6_HDR_LEN + SRH_FIXED_LEN + (srh.hdr_ext_len as usize) * 8
    } else {
        return None;
    };
    ctx.load(inner_offset + IPV6_DST_OFFSET).ok()
}

/// Removes the outer headers of the given length following the Ethernet header.
///
/// The inner IPv6 packet is handled by the network stack afterwards. It takes over the
/// outer hop limit if that is lower: the kernel encapsulates the inner packet for the next
/// path without decrementing its hop limit and copies it into the new outer header, so the
/// hops on the paths have to be counted in the inner header. Otherwise a packet caught in
/// a transient routing loop would circle forever.
#[inline(always)]
fn decapsulate(ctx: &TcContext, outer_len: usize) -> Result<i32, i64> {
    let outer_hop_limit: u8 = ctx.load(ETH_HDR_LEN + IPV6_HOP_LIMIT_OFFSET)?;
    let inner_hop_limit_offset = ETH_HDR_LEN + outer_len + IPV6_HOP_LIMIT_OFFSET;
    let inner_hop_limit: u8 = ctx.load(inner_hop_limit_offset)?;
    if outer_hop_limit < inner_hop_limit {
        // the hop limit isn't covered by the checksums of the transport protocols
        ctx.store(inner_hop_limit_offset, &outer_hop_limit, 0)?;
    }
    ctx.adjust_room(-(outer_len as i32), BPF_ADJ_ROOM_MAC, 0)?;
    Ok(TC_ACT_OK as i32)
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[unsafe(link_section = "license")]
#[unsafe(no_mangle)]
static LICENSE: [u8; 4] = *b"MIT\0";
