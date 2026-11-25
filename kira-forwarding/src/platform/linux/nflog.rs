//! Reception of the packets the forwarding tier dropped because their PathIP is unknown.
//!
//! The nftables rules of the GRE forwarding log these packets to the NFLOG group
//! [DROP_NFLOG_GROUP] (see [ForwardingNetlink](super::netlink::ForwardingNetlink)). The
//! routing tier can then notify the node that encapsulated the packet with an obsolete path.

use std::net::Ipv6Addr;

use futures::{
    StreamExt,
    future::{
        Either,
        select,
    },
    pin_mut,
};
use kira_r2kad::domain::IpPrefixes;
pub use kira_r2kad::domain::protocol_event::DroppedPacket;
use netlink_packet_core::{
    NetlinkMessage,
    NetlinkPayload,
};
use netlink_packet_netfilter::{
    NetfilterMessage,
    NetfilterMessageInner,
    NetfilterProtoFamily,
    nflog::{
        ConfigCmd,
        ConfigMode,
        ConfigNla,
        PacketNla,
        ULogMessage,
        config_request,
    },
};
use netlink_proto::{
    new_connection,
    sys::{
        SocketAddr,
        protocols::NETLINK_NETFILTER,
    },
};
use tokio::sync::mpsc::{
    Sender,
    error::TrySendError,
};

use super::netlink::{
    DROP_NFLOG_GROUP,
    DROP_NFLOG_SNAPLEN,
};

const IPV6_HEADER_LEN: usize = 40;
const IPPROTO_GRE: u8 = 47;
const ETHERTYPE_IPV6: u16 = 0x86dd;
const GRE_FLAG_CHECKSUM: u16 = 0x8000;
const GRE_FLAG_KEY: u16 = 0x2000;
const GRE_FLAG_SEQUENCE: u16 = 0x1000;

/// Destination address of the IPv6 header at the start of `header`.
fn ipv6_destination(header: &[u8]) -> Option<Ipv6Addr> {
    let bytes: [u8; 16] = header.get(24..40)?.try_into().ok()?;
    Some(Ipv6Addr::from(bytes))
}

/// Parses a dropped packet: the outer IPv6 header is followed by a GRE header and the
/// encapsulated IPv6 header.
///
/// Returns [None] if the source is no NodeIP or the destination no PathIP.
pub fn parse_dropped_packet(packet: &[u8], ip_prefixes: &IpPrefixes) -> Option<DroppedPacket> {
    if packet.len() < IPV6_HEADER_LEN || packet[0] >> 4 != 6 {
        return None;
    }
    let source: [u8; 16] = packet[8..24].try_into().ok()?;
    let encapsulator = ip_prefixes.node_id(&Ipv6Addr::from(source))?;
    let path_ip = ipv6_destination(packet)?;
    if !ip_prefixes.is_path_ip(&path_ip) {
        return None;
    }

    // the encapsulated packet (no extension headers are used by the forwarding tier)
    let destination = (packet[6] == IPPROTO_GRE)
        .then(|| {
            let gre = packet.get(IPV6_HEADER_LEN..IPV6_HEADER_LEN + 4)?;
            let flags = u16::from_be_bytes([gre[0], gre[1]]);
            if u16::from_be_bytes([gre[2], gre[3]]) != ETHERTYPE_IPV6 {
                return None;
            }
            let optional_fields = [GRE_FLAG_CHECKSUM, GRE_FLAG_KEY, GRE_FLAG_SEQUENCE]
                .iter()
                .filter(|flag| flags & **flag != 0)
                .count();
            let inner = packet.get(IPV6_HEADER_LEN + 4 + 4 * optional_fields..)?;
            ip_prefixes.node_id(&ipv6_destination(inner)?)
        })
        .flatten();

    Some(DroppedPacket {
        encapsulator,
        path_ip,
        destination,
    })
}

/// Error of [listen_dropped_packets].
#[derive(Debug, derive_more::Display, derive_more::Error, derive_more::From)]
pub enum NflogError {
    /// The netlink socket couldn't be created.
    #[display("netlink connection failed: {_0}")]
    Io(std::io::Error),
    /// The NFLOG group couldn't be bound.
    #[display("binding the NFLOG group failed: {_0}")]
    Bind(#[error(not(source))] String),
    /// The netlink connection was closed.
    #[display("netlink connection closed")]
    Closed,
}

/// Receives the packets the forwarding tier logs to [DROP_NFLOG_GROUP] and sends them to
/// `dropped` until the channel is closed.
///
/// Packets are discarded while the channel is full: the routing tier only needs to learn
/// about obsolete paths, not about every packet.
///
/// Fails if the group can't be bound, e.g., without `nfnetlink_log` support of the kernel.
pub async fn listen_dropped_packets(
    ip_prefixes: IpPrefixes,
    dropped: Sender<DroppedPacket>,
) -> Result<(), NflogError> {
    let (connection, handle, mut messages) = new_connection::<NetfilterMessage>(NETLINK_NETFILTER)?;

    let receive = async move {
        let bind = config_request(
            NetfilterProtoFamily::IPv6,
            DROP_NFLOG_GROUP,
            vec![
                ConfigCmd::Bind.into(),
                ConfigMode::new_packet(DROP_NFLOG_SNAPLEN).into(),
                // deliver every packet right away instead of batching them for up to a second
                ConfigNla::QThresh(1),
            ],
        );
        let mut response = handle
            .request(bind, SocketAddr::new(0, 0))
            .map_err(|err| NflogError::Bind(err.to_string()))?;
        while let Some(message) = response.next().await {
            if let NetlinkPayload::Error(err) = message.payload
                && err.code.is_some()
            {
                return Err(NflogError::Bind(err.to_string()));
            }
        }
        log::debug!(target: "native_fwd_table::nflog", "Listening to NFLOG group {DROP_NFLOG_GROUP}");

        while let Some((message, _)) = messages.next().await {
            let Some(packet) = payload(&message) else {
                continue;
            };
            match parse_dropped_packet(packet, &ip_prefixes) {
                Some(dropped_packet) => {
                    log::debug!(target: "native_fwd_table::nflog", "Dropped packet with unknown PathIP: {dropped_packet:?}");
                    match dropped.try_send(dropped_packet) {
                        Ok(()) | Err(TrySendError::Full(_)) => {}
                        Err(TrySendError::Closed(_)) => return Ok(()),
                    }
                }
                None => {
                    log::debug!(target: "native_fwd_table::nflog", "Ignoring logged packet of {} bytes", packet.len())
                }
            }
        }
        Err(NflogError::Closed)
    };

    // the connection has to be driven while receiving
    pin_mut!(connection, receive);
    match select(connection, receive).await {
        Either::Left(_) => Err(NflogError::Closed),
        Either::Right((result, _)) => result,
    }
}

/// The packet of an NFLOG message.
fn payload(message: &NetlinkMessage<NetfilterMessage>) -> Option<&[u8]> {
    let NetlinkPayload::InnerMessage(NetfilterMessage {
        inner: NetfilterMessageInner::ULog(ULogMessage::Packet(nlas)),
        ..
    }) = &message.payload
    else {
        return None;
    };
    nlas.iter().find_map(|nla| match nla {
        PacketNla::Payload(payload) => Some(payload.as_slice()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use kira_r2kad::domain::{
        NodeId,
        PathId,
    };

    use super::*;

    fn ipv6_header(next_header: u8, source: Ipv6Addr, destination: Ipv6Addr) -> Vec<u8> {
        let mut header = vec![0x60, 0, 0, 0, 0, 0, next_header, 64];
        header.extend(source.octets());
        header.extend(destination.octets());
        header
    }

    fn setup() -> (IpPrefixes, NodeId, Ipv6Addr, NodeId) {
        let prefixes = IpPrefixes::default();
        let encapsulator = NodeId::with_msb(3);
        let path_ip = prefixes.path_ip(&PathId::from_shake256(&[NodeId::with_msb(4)]));
        (prefixes, encapsulator, path_ip, NodeId::with_msb(5))
    }

    #[test]
    fn gre_packet_is_parsed() {
        let (prefixes, encapsulator, path_ip, destination) = setup();
        let mut packet = ipv6_header(IPPROTO_GRE, prefixes.node_ip(&encapsulator), path_ip);
        packet.extend([0, 0, 0x86, 0xdd]);
        packet.extend(ipv6_header(
            58,
            prefixes.node_ip(&encapsulator),
            prefixes.node_ip(&destination),
        ));

        assert_eq!(
            parse_dropped_packet(&packet, &prefixes),
            Some(DroppedPacket {
                encapsulator,
                path_ip,
                destination: Some(destination),
            })
        );
    }

    #[test]
    fn optional_gre_fields_are_skipped() {
        let (prefixes, encapsulator, path_ip, destination) = setup();
        let mut packet = ipv6_header(IPPROTO_GRE, prefixes.node_ip(&encapsulator), path_ip);
        // key and sequence number
        packet.extend([0x30, 0, 0x86, 0xdd, 0, 0, 0, 1, 0, 0, 0, 2]);
        packet.extend(ipv6_header(
            58,
            prefixes.node_ip(&encapsulator),
            prefixes.node_ip(&destination),
        ));

        assert_eq!(
            parse_dropped_packet(&packet, &prefixes)
                .unwrap()
                .destination,
            Some(destination)
        );
    }

    #[test]
    fn truncated_inner_packet_has_no_destination() {
        let (prefixes, encapsulator, path_ip, _) = setup();
        let mut packet = ipv6_header(IPPROTO_GRE, prefixes.node_ip(&encapsulator), path_ip);
        packet.extend([0, 0, 0x86, 0xdd, 0x60]);

        let dropped = parse_dropped_packet(&packet, &prefixes).unwrap();
        assert_eq!(dropped.encapsulator, encapsulator);
        assert_eq!(dropped.destination, None);
    }

    #[test]
    fn packets_not_from_node_ips_or_to_path_ips_are_ignored() {
        let (prefixes, encapsulator, path_ip, destination) = setup();
        let link_local: Ipv6Addr = "fe80::1".parse().unwrap();
        let from_link_local = ipv6_header(IPPROTO_GRE, link_local, path_ip);
        let to_node_ip = ipv6_header(
            IPPROTO_GRE,
            prefixes.node_ip(&encapsulator),
            prefixes.node_ip(&destination),
        );

        assert_eq!(parse_dropped_packet(&from_link_local, &prefixes), None);
        assert_eq!(parse_dropped_packet(&to_node_ip, &prefixes), None);
        assert_eq!(parse_dropped_packet(&[0x60; 10], &prefixes), None);
    }
}
