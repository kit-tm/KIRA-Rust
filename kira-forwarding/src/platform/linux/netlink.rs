//! Low level building blocks for managing the Linux kernel's routing.
//!
//! The implementation leverages the Netlink protocol[^nlproto].
//!
//! [NativeFwdTables]: crate::tables::native_tables::NativeFwdTables
//! [^nlproto]: See [`netlink(7)`](https://man7.org/linux/man-pages/man7/netlink.7.html)

use std::{
    net::Ipv6Addr,
    num::NonZeroU32,
};

use derive_more::derive::{
    Display,
    Error,
    From,
};
use futures::StreamExt;
use netlink_packet_core::{
    ErrorMessage,
    NLM_F_ACK,
    NLM_F_APPEND,
    NLM_F_CREATE,
    NLM_F_DUMP,
    NLM_F_EXCL,
    NLM_F_REPLACE,
    NLM_F_REQUEST,
    NetlinkHeader,
    NetlinkMessage,
    NetlinkPayload,
};
use netlink_packet_netfilter::{
    NetfilterHeader,
    NetfilterMessage,
    NetfilterProtoFamily,
    NetfilterSubsystem,
    nftables::{
        ChainAttribute,
        ChainMessage,
        ChecksumFlags,
        Cmp,
        DataAttribute,
        ExpressionAttribute,
        Expressions,
        Hook,
        Immediate,
        InetHookNumber,
        ListAttribute,
        Lookup,
        Meta,
        MetaKey,
        NfTablesMessage,
        Operator,
        Payload,
        Register,
        RuleAttribute,
        RuleMessage,
        SetAttribute,
        SetElementAttribute,
        SetElementList,
        SetElementMessage,
        SetFlags,
        SetMessage,
        TableAttribute,
        TableMessage,
        Verdict,
        VerdictAttribute,
    },
    none::ControlMessage,
};
use netlink_packet_route::{
    AddressFamily,
    RouteNetlinkMessage,
    address::{
        AddressAttribute,
        AddressHeader,
        AddressMessage,
    },
    link::{
        AfSpecInet6,
        AfSpecUnspec,
        In6AddrGenMode,
        InfoData,
        InfoGre6,
        InfoKind,
        LinkAttribute,
        LinkFlags,
        LinkHeader,
        LinkInfo,
        LinkMessage,
    },
    route::{
        RouteAddress,
        RouteAttribute,
        RouteHeader,
        RouteIp6Tunnel,
        RouteLwEnCapType,
        RouteLwTunnelEncap,
        RouteMessage,
        RouteProtocol,
    },
};
use netlink_proto::{
    ConnectionHandle,
    ReplyExpectation,
    sys::SocketAddr,
};
pub use netlink_proto::{
    new_connection,
    sys::protocols::{
        NETLINK_NETFILTER,
        NETLINK_ROUTE,
    },
};

use crate::{
    domain::{
        InterfaceId,
        IpPrefixes,
        NodeId,
        NodeIdSubnet,
        PathId,
    },
    underlay::UnderlayNeighborInformation,
};

#[derive(Debug, Clone)]
/// Manages links, addresses and routes using Netlink for the
/// [Native Forwarding Tables](crate::tables::native_tables).
///
/// Additionally a Netfilter Netlink component is required for realizing the Nftables rules
/// that manage the KIRA GRE encapsulation interface.
pub struct ForwardingNetlink {
    handle_rtnetlink: ConnectionHandle<RouteNetlinkMessage>,
    handle_nfnetlink: ConnectionHandle<NetfilterMessage>,
    kira: InterfaceId,
    ip_prefixes: IpPrefixes,
}

/// Default name for the [encap interface](create_encap_interface).
pub const KIRA_INTERFACE_NAME: &str = "kira";

const KIRA_TABLE: &str = "kira";
const KIRA_FORWARDMAP: &str = "forwardmap";

#[allow(non_upper_case_globals)]
const ICMPv6_PROTO: u8 = 0x3a;
const TYPE_IP6ADDR: u32 = 8;

#[derive(Debug, Display, Error, From)]
/// Error type used in [ForwardingNetlink].
pub enum ForwardingNetlinkError {
    /// Failed to deliver the RtNetlink message.
    #[display("Delivering the Netlink message failed: {_0}")]
    NetlinkNetlinkDeliveryFailure(netlink_proto::NetlinkProtoError),
    #[display("Received netlink error as response: {_0:?}")]
    /// The netlink socket responded with an [ErrorMessage] to the request.
    NetlinkResponseError(#[error(ignore)] ErrorMessage),
}

/// Result used by [ForwardingNetlink].
pub type Result<T> = std::result::Result<T, ForwardingNetlinkError>;

impl ForwardingNetlink {
    /// Creates a new [ForwardingNetlink] instance.
    ///
    /// As the interface name the [default name](KIRA_INTERFACE_NAME) is used.
    /// NodeIPs and PathIPs are derived using the given `ip_prefixes`.
    pub async fn new(
        handle_rtnetlink: ConnectionHandle<RouteNetlinkMessage>,
        handle_nfnetlink: ConnectionHandle<NetfilterMessage>,
        ip_prefixes: IpPrefixes,
    ) -> Result<Self> {
        // a previous instance (e.g., after a crash) may have left its state behind
        remove_forwarding_state(&handle_rtnetlink, &handle_nfnetlink, &ip_prefixes).await?;
        let kira = create_encap_interface(&handle_rtnetlink, KIRA_INTERFACE_NAME).await?;
        Self::with_kira_interface(handle_rtnetlink, handle_nfnetlink, kira, ip_prefixes).await
    }

    /// Creates a new [ForwardingNetlink] instance with the given interface.
    ///
    /// You must ensure that the supplied `kira` [InterfaceId] points to
    /// a valid encap interface. You can create one using [create_encap_interface].
    pub async fn with_kira_interface(
        handle_rtnetlink: ConnectionHandle<RouteNetlinkMessage>,
        handle_nfnetlink: ConnectionHandle<NetfilterMessage>,
        kira: InterfaceId,
        ip_prefixes: IpPrefixes,
    ) -> Result<Self> {
        create_initial_nft_rules(
            &handle_nfnetlink,
            KIRA_TABLE.to_string(),
            "hookin".to_string(),
            "forwarding".to_string(),
            KIRA_FORWARDMAP.to_string(),
            &ip_prefixes,
        )
        .await?;

        Ok(Self {
            handle_rtnetlink,
            handle_nfnetlink,
            kira,
            ip_prefixes,
        })
    }

    /// Returns the prefixes used to derive NodeIPs and PathIPs.
    pub fn ip_prefixes(&self) -> &IpPrefixes {
        &self.ip_prefixes
    }

    /// Encapsulates packets with a destination ip of `node_id`
    /// using the encapsulation device `kira` and sets the new outer destination to `path_id`.
    ///
    /// This rule is used for realizing an [NodeIdEncapsulationEntry].
    /// An existing rule is overwritten.
    ///
    /// [NodeIdEncapsulationEntry]: crate::domain::NodeIdEncapsulationEntry
    #[tracing::instrument(
        level = "trace",
        target = "native_fwd_table::netlink",
        skip_all,
        fields(%node_id, out_path_id=%path_id)
    )]
    pub async fn replace_encap_route(
        &self,
        node_id: &NodeIdSubnet,
        path_id: &PathId,
    ) -> Result<()> {
        let (node_ip, prefix_length) = self.ip_prefixes.node_ip_subnet(node_id);

        let mut nl_hdr = NetlinkHeader::default();
        nl_hdr.flags = NLM_F_REQUEST | NLM_F_CREATE | NLM_F_REPLACE | NLM_F_ACK;

        let mut rt_msg = RouteMessage::default();
        rt_msg.header = RouteHeader {
            address_family: AddressFamily::Inet6,
            destination_prefix_length: prefix_length,
            protocol: RouteProtocol::Static,
            ..rt_msg.header
        };
        rt_msg
            .attributes
            .push(RouteAttribute::Destination(RouteAddress::Inet6(node_ip)));
        rt_msg
            .attributes
            .push(RouteAttribute::Oif(self.kira.into()));
        rt_msg
            .attributes
            .push(RouteAttribute::EncapType(RouteLwEnCapType::Ip6));
        rt_msg
            .attributes
            .push(RouteAttribute::Encap(vec![RouteLwTunnelEncap::Ip6(
                RouteIp6Tunnel::Destination(self.ip_prefixes.path_ip(path_id)),
            )]));

        let msg = NetlinkMessage::new(nl_hdr, RouteNetlinkMessage::NewRoute(rt_msg).into());

        let mut response = self.handle_rtnetlink.request(msg, SocketAddr::new(0, 0))?;

        while let Some(response) = response.next().await {
            if let NetlinkPayload::Error(err) = response.payload {
                return Err(err.into());
            }
        }

        log::debug!(target: "native_fwd_table::netlink", "encap route replaced: {:?} encap {:?} ", node_id, path_id);

        Ok(())
    }

    /// Stops encapsulating packets with destination `node_id`.
    #[tracing::instrument(
        level = "trace",
        target = "native_fwd_table::netlink",
        skip_all,
        fields(%node_id,
        out_path_id=%path_id)
    )]
    pub async fn delete_encap_route(&self, node_id: &NodeIdSubnet, path_id: &PathId) -> Result<()> {
        let (node_ip, prefix_length) = self.ip_prefixes.node_ip_subnet(node_id);

        let mut nl_hdr = NetlinkHeader::default();
        nl_hdr.flags = NLM_F_REQUEST | NLM_F_ACK;

        let mut rt_msg = RouteMessage::default();
        rt_msg.header = RouteHeader {
            address_family: AddressFamily::Inet6,
            destination_prefix_length: prefix_length,
            protocol: RouteProtocol::Static,
            ..rt_msg.header
        };
        rt_msg
            .attributes
            .push(RouteAttribute::Destination(RouteAddress::Inet6(node_ip)));
        rt_msg
            .attributes
            .push(RouteAttribute::Oif(self.kira.into()));
        rt_msg
            .attributes
            .push(RouteAttribute::EncapType(RouteLwEnCapType::Ip6));
        rt_msg
            .attributes
            .push(RouteAttribute::Encap(vec![RouteLwTunnelEncap::Ip6(
                RouteIp6Tunnel::Destination(self.ip_prefixes.path_ip(path_id)),
            )]));

        let msg = NetlinkMessage::new(nl_hdr, RouteNetlinkMessage::DelRoute(rt_msg).into());

        let mut response = self.handle_rtnetlink.request(msg, SocketAddr::new(0, 0))?;

        while let Some(response) = response.next().await {
            if let NetlinkPayload::Error(err) = response.payload {
                return Err(err.into());
            }
        }

        log::debug!(target: "native_fwd_table::netlink", "encap route deleted: {:?} encap {:?} ", node_id, path_id);

        Ok(())
    }

    /// Forwards packets with destination `path_ip` as if they where destined to `next_hop_ip`.
    ///
    /// This rule is used to determine to which underlay neighbor (`next_hop_ip`)
    /// a packet with a given [PathId] is forwarded.
    /// This rule is used in combination with a [forwarding_rule] to realize A [PathIdForwardingEntry].
    /// An existing rule is overwritten.
    ///
    /// [PathIdForwardingEntry]: crate::domain::PathIdForwardingEntry
    /// [forwarding_rule]: Self::add_forwarding_rule
    #[tracing::instrument(
        level = "trace",
        target = "native_fwd_table::netlink",
        skip_all,
        fields(%path_id, ?next_hop)
    )]
    // TODO: figure out where route should get deleted
    pub async fn replace_via_route(
        &self,
        path_id: &PathId,
        next_hop: &UnderlayNeighborInformation,
    ) -> Result<()> {
        let mut nl_hdr = NetlinkHeader::default();
        nl_hdr.flags = NLM_F_REQUEST | NLM_F_CREATE | NLM_F_REPLACE | NLM_F_ACK;

        let mut rt_msg = RouteMessage::default();
        rt_msg.header = RouteHeader {
            address_family: AddressFamily::Inet6,
            destination_prefix_length: 128,
            protocol: RouteProtocol::Static,
            ..rt_msg.header
        };
        rt_msg.attributes = vec![
            RouteAttribute::Destination(RouteAddress::Inet6(self.ip_prefixes.path_ip(path_id))),
            RouteAttribute::Gateway(RouteAddress::Inet6(next_hop.ll_ipv6)),
            RouteAttribute::Oif(next_hop.interface_id.into()),
        ];

        let msg = NetlinkMessage::new(nl_hdr, RouteNetlinkMessage::NewRoute(rt_msg).into());

        let mut response = self.handle_rtnetlink.request(msg, SocketAddr::new(0, 0))?;

        while let Some(response) = response.next().await {
            if let NetlinkPayload::Error(err) = response.payload {
                return Err(err.into());
            }
        }

        log::debug!(target: "native_fwd_table::netlink", "route replaced: {:?} via {}%{}", path_id, next_hop.ll_ipv6, next_hop.interface_id);

        Ok(())
    }

    /// Forwards packets with destination `ip` to interface with name `interface_name` unchanged.
    ///
    /// This is used to realize [NodeIdForwardingEntry].
    ///
    /// [NodeIdForwardingEntry]: crate::domain::NodeIdForwardingEntry
    #[tracing::instrument(
        level = "trace",
        target = "native_fwd_table::netlink",
        skip_all,
        fields(%node_id, interface=%interface_id)
    )]
    pub async fn replace_neighbor_route(
        &self,
        node_id: &NodeIdSubnet,
        interface_id: InterfaceId,
    ) -> Result<()> {
        let (node_ip, prefix_length) = self.ip_prefixes.node_ip_subnet(node_id);

        let mut nl_hdr = NetlinkHeader::default();
        nl_hdr.flags = NLM_F_REQUEST | NLM_F_CREATE | NLM_F_REPLACE | NLM_F_ACK;

        let mut rt_msg = RouteMessage::default();
        rt_msg.header = RouteHeader {
            address_family: AddressFamily::Inet6,
            destination_prefix_length: prefix_length,
            protocol: RouteProtocol::Static,
            ..rt_msg.header
        };
        rt_msg
            .attributes
            .push(RouteAttribute::Destination(RouteAddress::Inet6(node_ip)));
        rt_msg
            .attributes
            .push(RouteAttribute::Oif(interface_id.into()));

        let msg = NetlinkMessage::new(nl_hdr, RouteNetlinkMessage::NewRoute(rt_msg).into());

        let mut response = self.handle_rtnetlink.request(msg, SocketAddr::new(0, 0))?;

        while let Some(response) = response.next().await {
            if let NetlinkPayload::Error(err) = response.payload {
                return Err(err.into());
            }
        }

        log::debug!(target: "native_fwd_table::netlink", "route replaced: {} dev {}", node_ip, interface_id);

        Ok(())
    }

    /// Deletes a [neighbor route].
    ///
    /// [neighbor route]: Self::replace_neighbor_route
    #[tracing::instrument(
        level = "trace",
        target = "native_fwd_table::netlink",
        skip_all,
        fields(%node_id, interface=%interface_id)
    )]
    pub async fn delete_neighbor_route(
        &self,
        node_id: &NodeIdSubnet,
        interface_id: InterfaceId,
    ) -> Result<()> {
        let (node_ip, prefix_length) = self.ip_prefixes.node_ip_subnet(node_id);

        let mut nl_hdr = NetlinkHeader::default();
        nl_hdr.flags = NLM_F_REQUEST | NLM_F_ACK;

        let mut rt_msg = RouteMessage::default();
        rt_msg.header = RouteHeader {
            address_family: AddressFamily::Inet6,
            destination_prefix_length: prefix_length,
            protocol: RouteProtocol::Static,
            ..rt_msg.header
        };
        rt_msg
            .attributes
            .push(RouteAttribute::Destination(RouteAddress::Inet6(node_ip)));
        rt_msg
            .attributes
            .push(RouteAttribute::Oif(interface_id.into()));

        let msg = NetlinkMessage::new(nl_hdr, RouteNetlinkMessage::DelRoute(rt_msg).into());

        let mut response = self.handle_rtnetlink.request(msg, SocketAddr::new(0, 0))?;

        while let Some(response) = response.next().await {
            if let NetlinkPayload::Error(err) = response.payload {
                return Err(err.into());
            }
        }

        log::debug!(target: "native_fwd_table::netlink", "route deleted: {} dev {}", node_id, interface_id);

        Ok(())
    }

    /// Adds a destination based forwarding rule.
    ///
    /// This rule forwards IPv6 packets destined to `from` using the new destination.
    #[tracing::instrument(
        level = "trace",
        target = "native_fwd_table::netlink",
        skip_all,
        fields(%from, %to)
    )]
    pub async fn add_forwarding_rule(&self, from: Ipv6Addr, to: Ipv6Addr) -> Result<()> {
        let mut batch_begin = NetlinkMessage::from(NetfilterMessage::new(
            NetfilterHeader::new(
                NetfilterProtoFamily::Unspec,
                0,
                u8::from(NetfilterSubsystem::NfTables) as _,
            ),
            ControlMessage::BatchBegin,
        ));
        batch_begin.header.flags = NLM_F_REQUEST;

        let mut msg = NetlinkMessage::from(NetfilterMessage::new(
            NetfilterHeader::new(NetfilterProtoFamily::IPv6, 0, 0),
            NfTablesMessage::NewSetElement(SetElementMessage {
                attributes: vec![
                    SetElementList::Table(KIRA_TABLE.to_string()),
                    SetElementList::Set(KIRA_FORWARDMAP.to_string()),
                    SetElementList::Elements(vec![ListAttribute::Element(vec![
                        SetElementAttribute::Key(DataAttribute::Value(from.octets().to_vec())),
                        SetElementAttribute::Data(DataAttribute::Value(to.octets().to_vec())),
                    ])]),
                ],
            }),
        ));
        msg.header.flags = NLM_F_REQUEST | NLM_F_CREATE | NLM_F_ACK;

        let mut batch_end = NetlinkMessage::from(NetfilterMessage::new(
            NetfilterHeader::new(
                NetfilterProtoFamily::Unspec,
                0,
                u8::from(NetfilterSubsystem::NfTables) as _,
            ),
            ControlMessage::BatchEnd,
        ));
        batch_end.header.flags = NLM_F_REQUEST;

        let batch = [
            (batch_begin, ReplyExpectation::NotExpected),
            (msg, ReplyExpectation::Expected),
            (batch_end, ReplyExpectation::NotExpected),
        ];
        let mut response = self
            .handle_nfnetlink
            .request_batch_with_expectation(batch, SocketAddr::new(0, 0))?;

        while let Some(response) = response.next().await {
            if let NetlinkPayload::Error(err) = response.payload {
                return Err(err.into());
            }
        }

        log::debug!(
            target: "native_fwd_table::netlink",
            "command \"adding element to forwarding map {{\"{from}\" : \"{to}\"}}\" succeeded"
        );

        Ok(())
    }

    /// Updates an existing forwarding rule by first deleting it.
    ///
    /// See [Self::add_forwarding_rule] for more details.
    #[tracing::instrument(
        level = "trace",
        target = "native_fwd_table::netlink",
        skip_all,
        fields(%from, %to)
    )]
    pub async fn update_forwarding_rule(&self, from: Ipv6Addr, to: Ipv6Addr) -> Result<()> {
        let mut batch_begin = NetlinkMessage::from(NetfilterMessage::new(
            NetfilterHeader::new(
                NetfilterProtoFamily::Unspec,
                0,
                u8::from(NetfilterSubsystem::NfTables) as _,
            ),
            ControlMessage::BatchBegin,
        ));
        batch_begin.header.flags = NLM_F_REQUEST;

        let mut msg_delete = NetlinkMessage::from(NetfilterMessage::new(
            NetfilterHeader::new(NetfilterProtoFamily::IPv6, 0, 0),
            NfTablesMessage::DeleteSetElement(SetElementMessage {
                attributes: vec![
                    SetElementList::Table(KIRA_TABLE.to_string()),
                    SetElementList::Set(KIRA_FORWARDMAP.to_string()),
                    SetElementList::Elements(vec![ListAttribute::Element(vec![
                        SetElementAttribute::Key(DataAttribute::Value(from.octets().to_vec())),
                    ])]),
                ],
            }),
        ));
        msg_delete.header.flags = NLM_F_REQUEST | NLM_F_ACK;

        let mut msg_new = NetlinkMessage::from(NetfilterMessage::new(
            NetfilterHeader::new(NetfilterProtoFamily::IPv6, 0, 0),
            NfTablesMessage::NewSetElement(SetElementMessage {
                attributes: vec![
                    SetElementList::Table(KIRA_TABLE.to_string()),
                    SetElementList::Set(KIRA_FORWARDMAP.to_string()),
                    SetElementList::Elements(vec![ListAttribute::Element(vec![
                        SetElementAttribute::Key(DataAttribute::Value(from.octets().to_vec())),
                        SetElementAttribute::Data(DataAttribute::Value(to.octets().to_vec())),
                    ])]),
                ],
            }),
        ));
        msg_new.header.flags = NLM_F_REQUEST | NLM_F_CREATE | NLM_F_EXCL | NLM_F_ACK;

        let mut batch_end = NetlinkMessage::from(NetfilterMessage::new(
            NetfilterHeader::new(
                NetfilterProtoFamily::Unspec,
                0,
                u8::from(NetfilterSubsystem::NfTables) as _,
            ),
            ControlMessage::BatchEnd,
        ));
        batch_end.header.flags = NLM_F_REQUEST;

        let batch = [
            (batch_begin, ReplyExpectation::NotExpected),
            (msg_delete, ReplyExpectation::Expected),
            (msg_new, ReplyExpectation::Expected),
            (batch_end, ReplyExpectation::NotExpected),
        ];
        let mut response = self
            .handle_nfnetlink
            .request_batch_with_expectation(batch, SocketAddr::new(0, 0))?;

        while let Some(response) = response.next().await {
            if let NetlinkPayload::Error(err) = response.payload {
                return Err(err.into());
            }
        }

        log::debug!(
            target: "native_fwd_table::netlink",
            "command \"updating element to forwarding map {{\"{from}\" : \"{to}\"}}\" succeeded"
        );

        Ok(())
    }

    /// Deletes an existing forwarding rule.
    #[tracing::instrument(
        level = "trace",
        target = "native_fwd_table::netlink",
        skip_all,
        fields(%from)
    )]
    pub async fn delete_forwarding_rule(&self, from: Ipv6Addr) -> Result<()> {
        let mut batch_begin = NetlinkMessage::from(NetfilterMessage::new(
            NetfilterHeader::new(
                NetfilterProtoFamily::Unspec,
                0,
                u8::from(NetfilterSubsystem::NfTables) as _,
            ),
            ControlMessage::BatchBegin,
        ));
        batch_begin.header.flags = NLM_F_REQUEST;

        let mut msg = NetlinkMessage::from(NetfilterMessage::new(
            NetfilterHeader::new(NetfilterProtoFamily::IPv6, 0, 0),
            NfTablesMessage::DeleteSetElement(SetElementMessage {
                attributes: vec![
                    SetElementList::Table(KIRA_TABLE.to_string()),
                    SetElementList::Set(KIRA_FORWARDMAP.to_string()),
                    SetElementList::Elements(vec![ListAttribute::Element(vec![
                        SetElementAttribute::Key(DataAttribute::Value(from.octets().to_vec())),
                    ])]),
                ],
            }),
        ));
        msg.header.flags = NLM_F_REQUEST | NLM_F_ACK;

        let mut batch_end = NetlinkMessage::from(NetfilterMessage::new(
            NetfilterHeader::new(
                NetfilterProtoFamily::Unspec,
                0,
                u8::from(NetfilterSubsystem::NfTables) as _,
            ),
            ControlMessage::BatchEnd,
        ));
        batch_end.header.flags = NLM_F_REQUEST;

        let batch = [
            (batch_begin, ReplyExpectation::NotExpected),
            (msg, ReplyExpectation::Expected),
            (batch_end, ReplyExpectation::NotExpected),
        ];
        let mut response = self
            .handle_nfnetlink
            .request_batch_with_expectation(batch, SocketAddr::new(0, 0))?;

        while let Some(response) = response.next().await {
            if let NetlinkPayload::Error(err) = response.payload {
                return Err(err.into());
            }
        }

        log::debug!(
            target: "native_fwd_table::netlink",
            "command \"removing element to forwarding map {{\"{from}\" : \"???\"}}\" succeeded"
        );

        Ok(())
    }

    /// Attaches the corresponding IPv6 address of `node_id` to the interface with name `interface`.
    ///
    /// The `node_id` usually is the root-id of KIRA instance running on the node.
    #[tracing::instrument(level = "trace", target = "native_fwd_table::netlink")]
    pub async fn attach_node_id_ip(
        &self,
        node_id: &NodeId,
        interface_id: InterfaceId,
    ) -> Result<()> {
        let node_ip = self.ip_prefixes.node_ip(node_id);

        let mut nl_hdr = NetlinkHeader::default();
        nl_hdr.flags = NLM_F_REQUEST | NLM_F_CREATE | NLM_F_REPLACE | NLM_F_ACK;

        let mut rt_msg = AddressMessage::default();
        rt_msg.header = AddressHeader {
            family: AddressFamily::Inet6,
            prefix_len: 128,
            index: interface_id.into(),
            ..rt_msg.header
        };
        rt_msg
            .attributes
            .push(AddressAttribute::Address(std::net::IpAddr::V6(node_ip)));

        let msg = NetlinkMessage::new(nl_hdr, RouteNetlinkMessage::NewAddress(rt_msg).into());

        let mut response = self.handle_rtnetlink.request(msg, SocketAddr::new(0, 0))?;

        while let Some(response) = response.next().await {
            if let NetlinkPayload::Error(err) = response.payload {
                return Err(err.into());
            }
        }

        log::debug!(target: "native_fwd_table::netlink", "added node-id ip to interface: {} dev {}", node_id, interface_id);

        Ok(())
    }

    /// Deletes the encapsulation interface.
    ///
    /// This method consumes the struct because other methods rely on the interface
    /// to be present.
    #[tracing::instrument(level = "trace", target = "native_fwd_table::netlink", skip_all)]
    pub async fn delete_encap_interface(self) -> Result<()> {
        let mut nl_hdr = NetlinkHeader::default();
        nl_hdr.flags = NLM_F_REQUEST | NLM_F_ACK;

        let mut rt_msg = LinkMessage::default();
        rt_msg.header = LinkHeader {
            index: self.kira.into(),
            ..rt_msg.header
        };

        let msg = NetlinkMessage::new(nl_hdr, RouteNetlinkMessage::DelLink(rt_msg).into());

        let mut response = self.handle_rtnetlink.request(msg, SocketAddr::new(0, 0))?;

        while let Some(response) = response.next().await {
            if let NetlinkPayload::Error(err) = response.payload {
                return Err(err.into());
            }
        }

        log::debug!(target: "native_fwd_table::netlink", "deleted encap interface: {:?}", self.kira);

        Ok(())
    }
}

/// Error codes of netlink answers to delete requests for things that don't exist.
const ENOENT: i32 = -2;
const ESRCH: i32 = -3;
const ENODEV: i32 = -19;

/// Sends the request and checks the answers, ignoring the given error codes.
async fn request_ignoring<T>(
    handle: &ConnectionHandle<T>,
    msg: NetlinkMessage<T>,
    ignored_codes: &[i32],
) -> Result<()>
where
    T: std::fmt::Debug
        + netlink_packet_core::NetlinkSerializable
        + netlink_packet_core::NetlinkDeserializable
        + Clone
        + Send
        + 'static,
{
    let mut response = handle.request(msg, SocketAddr::new(0, 0))?;
    while let Some(response) = response.next().await {
        if let NetlinkPayload::Error(err) = response.payload
            && let Some(code) = err.code
            && !ignored_codes.contains(&code.get())
        {
            return Err(err.into());
        }
    }
    Ok(())
}

/// Removes the forwarding state of KIRA: the encapsulation interface (and the routes via it),
/// the nftables table and the routes kirad added into the NodeIP and PathIP prefixes.
///
/// It is used on shutdown and on startup, in case a previous instance (e.g., after a crash)
/// left its state behind. Missing parts are ignored.
#[tracing::instrument(level = "trace", target = "native_fwd_table::netlink", skip_all)]
pub async fn remove_forwarding_state(
    handle_rtnetlink: &ConnectionHandle<RouteNetlinkMessage>,
    handle_nfnetlink: &ConnectionHandle<NetfilterMessage>,
    ip_prefixes: &IpPrefixes,
) -> Result<()> {
    // encapsulation interface, deleted by name
    {
        let mut nl_hdr = NetlinkHeader::default();
        nl_hdr.flags = NLM_F_REQUEST | NLM_F_ACK;
        let mut rt_msg = LinkMessage::default();
        rt_msg.attributes = vec![LinkAttribute::IfName(KIRA_INTERFACE_NAME.to_string())];
        let msg = NetlinkMessage::new(nl_hdr, RouteNetlinkMessage::DelLink(rt_msg).into());
        request_ignoring(handle_rtnetlink, msg, &[ENODEV]).await?;
    }

    // nftables table with its chains, rules and forwarding map
    {
        let control = |message| {
            NetlinkMessage::from(NetfilterMessage::new(
                NetfilterHeader::new(
                    NetfilterProtoFamily::Unspec,
                    0,
                    u8::from(NetfilterSubsystem::NfTables) as _,
                ),
                message,
            ))
        };
        let mut delete_table = NetlinkMessage::from(NetfilterMessage::new(
            NetfilterHeader::new(NetfilterProtoFamily::IPv6, 0, 0),
            NfTablesMessage::DeleteTable(TableMessage {
                attributes: vec![TableAttribute::Name(KIRA_TABLE.to_string())],
            }),
        ));
        delete_table.header.flags = NLM_F_REQUEST | NLM_F_ACK;
        let mut batch_begin = control(ControlMessage::BatchBegin);
        batch_begin.header.flags = NLM_F_REQUEST;
        let mut batch_end = control(ControlMessage::BatchEnd);
        batch_end.header.flags = NLM_F_REQUEST;
        let batch = [
            (batch_begin, ReplyExpectation::NotExpected),
            (delete_table, ReplyExpectation::Expected),
            (batch_end, ReplyExpectation::NotExpected),
        ];
        let mut response =
            handle_nfnetlink.request_batch_with_expectation(batch, SocketAddr::new(0, 0))?;
        while let Some(response) = response.next().await {
            if let NetlinkPayload::Error(err) = response.payload
                && let Some(code) = err.code
                && code.get() != ENOENT
            {
                return Err(err.into());
            }
        }
    }

    // routes into the NodeIP and PathIP prefixes added by kirad (via ULNs)
    let routes = {
        let mut nl_hdr = NetlinkHeader::default();
        nl_hdr.flags = NLM_F_REQUEST | NLM_F_DUMP;
        let mut rt_msg = RouteMessage::default();
        rt_msg.header.address_family = AddressFamily::Inet6;
        let msg = NetlinkMessage::new(nl_hdr, RouteNetlinkMessage::GetRoute(rt_msg).into());
        let mut response = handle_rtnetlink.request(msg, SocketAddr::new(0, 0))?;
        let prefixes = [
            ip_prefixes.node_ip_prefix().bits(),
            ip_prefixes.path_ip_prefix().bits(),
        ];
        let mut routes = Vec::new();
        while let Some(response) = response.next().await {
            match response.payload {
                NetlinkPayload::Error(err) if err.code.is_some() => return Err(err.into()),
                NetlinkPayload::InnerMessage(RouteNetlinkMessage::NewRoute(route))
                    if route.header.protocol == RouteProtocol::Static
                        && route.header.table == RouteHeader::RT_TABLE_MAIN
                        && route.attributes.iter().any(|attribute| {
                            matches!(
                                attribute,
                                RouteAttribute::Destination(RouteAddress::Inet6(destination))
                                    if prefixes.contains(&destination.segments()[0])
                            )
                        }) =>
                {
                    routes.push(route);
                }
                _ => {}
            }
        }
        routes
    };
    for route in routes {
        let mut nl_hdr = NetlinkHeader::default();
        nl_hdr.flags = NLM_F_REQUEST | NLM_F_ACK;
        let msg = NetlinkMessage::new(nl_hdr, RouteNetlinkMessage::DelRoute(route).into());
        request_ignoring(handle_rtnetlink, msg, &[ESRCH]).await?;
    }

    log::debug!(target: "native_fwd_table::netlink", "removed KIRA's forwarding state");
    Ok(())
}

/// Creates an interface for encapsulating and decapsulating
/// IPv6 packets using [GRE](https://datatracker.ietf.org/doc/rfc7676/).
///
/// This interface is expected to be fully managed by [ForwardingNetlink].
#[tracing::instrument(level = "trace", target = "native_fwd_table::netlink", skip(handle))]
pub async fn create_encap_interface(
    handle: &ConnectionHandle<RouteNetlinkMessage>,
    if_name: &str,
) -> Result<InterfaceId> {
    // create interface
    log::trace!(target: "native_fwd_table::netlink", "create encap interface {}", if_name);
    {
        let mut nl_hdr = NetlinkHeader::default();
        nl_hdr.flags = NLM_F_REQUEST | NLM_F_CREATE | NLM_F_ACK | NLM_F_EXCL;

        let mut rt_msg = LinkMessage::default();
        rt_msg.header = LinkHeader {
            index: 0, // let the kernel decide on interface id
            ..rt_msg.header
        };
        rt_msg.attributes = vec![
            LinkAttribute::IfName(if_name.to_string()),
            LinkAttribute::LinkInfo(vec![
                LinkInfo::Kind(InfoKind::GreTun6),
                LinkInfo::Data(InfoData::GreTun6(vec![
                    InfoGre6::CollectMetadata, // externally managed (by Nftables)
                ])),
            ]),
        ];
        let msg = NetlinkMessage::new(nl_hdr, RouteNetlinkMessage::NewLink(rt_msg).into());

        let mut response = handle.request(msg, SocketAddr::new(0, 0))?;
        while let Some(response) = response.next().await {
            if let NetlinkPayload::Error(err) = response.payload {
                return Err(err.into());
            }
        }
    }

    let mut nl_hdr = NetlinkHeader::default();
    nl_hdr.flags = NLM_F_REQUEST | NLM_F_ACK;

    log::trace!(target: "native_fwd_table::netlink", "obtain interface index of encap interface");
    // get interface index
    let kira = {
        let mut rt_msg = LinkMessage::default();
        rt_msg.header = LinkHeader { ..rt_msg.header };
        rt_msg.attributes = vec![LinkAttribute::IfName(if_name.to_string())];
        let msg = NetlinkMessage::new(nl_hdr, RouteNetlinkMessage::GetLink(rt_msg).into());

        let mut response = handle.request(msg, SocketAddr::new(0, 0))?;

        let mut kira = None;
        while let Some(response) = response.next().await {
            match response.payload {
                NetlinkPayload::Error(err) => return Err(err.into()),
                NetlinkPayload::InnerMessage(RouteNetlinkMessage::NewLink(LinkMessage {
                    header,
                    ..
                })) => {
                    kira.replace(InterfaceId(
                        NonZeroU32::new(header.index).expect("non zero interface id"),
                    ));
                }
                _ => {}
            }
        }
        kira.expect("request should yield interface id")
    };
    log::trace!(target: "native_fwd_table::netlink", "Obtained interface id of interface {}: {}", if_name, kira);

    log::trace!(target: "native_fwd_table::netlink", "disable address generation mode on interface");
    // disable address generation mode on interface
    {
        let mut rt_msg = LinkMessage::default();
        rt_msg.header = LinkHeader {
            index: kira.into(),
            ..rt_msg.header
        };
        rt_msg.attributes = vec![LinkAttribute::AfSpecUnspec(vec![AfSpecUnspec::Inet6(
            vec![AfSpecInet6::AddrGenMode(In6AddrGenMode::None)],
        )])];
        let msg = NetlinkMessage::new(nl_hdr, RouteNetlinkMessage::SetLink(rt_msg).into());

        let mut response = handle.request(msg, SocketAddr::new(0, 0))?;
        while let Some(response) = response.next().await {
            if let NetlinkPayload::Error(err) = response.payload {
                return Err(err.into());
            }
        }
    }

    // set interface up
    {
        let mut rt_msg = LinkMessage::default();
        rt_msg.header = LinkHeader {
            index: kira.into(),
            flags: LinkFlags::Up,
            ..rt_msg.header
        };
        let msg = NetlinkMessage::new(nl_hdr, RouteNetlinkMessage::SetLink(rt_msg).into());

        let mut response = handle.request(msg, SocketAddr::new(0, 0))?;
        while let Some(response) = response.next().await {
            if let NetlinkPayload::Error(err) = response.payload {
                return Err(err.into());
            }
        }
    }

    log::debug!(target: "native_fwd_table::netlink", "created encap interface {}: {:?}", if_name, kira);

    Ok(kira)
}

async fn create_initial_nft_rules(
    handle_nfnetlink: &ConnectionHandle<NetfilterMessage>,
    table: String,
    hookin_chain: String,
    forwarding_chain: String,
    forwarding_map: String,
    ip_prefixes: &IpPrefixes,
) -> Result<()> {
    // Create table
    let create_new_table = NetlinkMessage::from(NetfilterMessage::new(
        NetfilterHeader::new(NetfilterProtoFamily::IPv6, 0, 0),
        NfTablesMessage::NewTable(TableMessage {
            attributes: vec![TableAttribute::Name(table.clone())],
        }),
    ));

    // Create chains
    let mut create_hookin_chain = NetlinkMessage::from(NetfilterMessage::new(
        NetfilterHeader::new(NetfilterProtoFamily::IPv6, 0, 0),
        NfTablesMessage::NewChain(ChainMessage {
            attributes: vec![
                ChainAttribute::Table(table.clone()),
                ChainAttribute::Name(hookin_chain.clone()),
                ChainAttribute::Policy(1), // accept
                ChainAttribute::Type("filter".to_string()),
                ChainAttribute::Hook(vec![
                    Hook::Number(InetHookNumber::PreRouting.into()),
                    Hook::Priority(-300i32 as _),
                ]),
            ],
        }),
    ));
    create_hookin_chain.header.flags |= NLM_F_CREATE;
    let mut create_forwarding_chain = NetlinkMessage::from(NetfilterMessage::new(
        NetfilterHeader::new(NetfilterProtoFamily::IPv6, 0, 0),
        NfTablesMessage::NewChain(ChainMessage {
            attributes: vec![
                ChainAttribute::Table(table.clone()),
                ChainAttribute::Name(forwarding_chain.clone()),
            ],
        }),
    ));
    create_forwarding_chain.header.flags |= NLM_F_CREATE;

    // Forwarding map holds PathIP/NodeIP translation
    let mut create_forwarding_map = NetlinkMessage::from(NetfilterMessage::new(
        NetfilterHeader::new(NetfilterProtoFamily::IPv6, 0, 0),
        NfTablesMessage::NewSet(SetMessage {
            attributes: vec![
                SetAttribute::Table(table.clone()),
                SetAttribute::Name(forwarding_map.clone()),
                SetAttribute::Flags(SetFlags::Map),
                SetAttribute::KeyType(TYPE_IP6ADDR),
                SetAttribute::KeyLen(size_of::<Ipv6Addr>() as _),
                SetAttribute::DataType(TYPE_IP6ADDR),
                SetAttribute::DataLen(size_of::<Ipv6Addr>() as _),
                SetAttribute::Id(1),
            ],
        }),
    ));
    create_forwarding_map.header.flags |= NLM_F_CREATE;

    fn filter_16_bit_prefix(prefix: u16) -> [ListAttribute<ExpressionAttribute>; 2] {
        [
            Expressions::Payload(vec![
                Payload::DestinationRegister(Register::Reg1),
                Payload::Base(1),    // start at IPv6 header
                Payload::Offset(24), // destination address
                Payload::Len(2),     // 16-bit prefix
            ])
            .into(),
            Expressions::Cmp(vec![
                Cmp::SourceRegister(Register::Reg1),
                Cmp::Op(Operator::Equal),
                Cmp::Data(DataAttribute::Value(prefix.to_be_bytes().to_vec())),
            ])
            .into(),
        ]
    }
    let goto_forwarding_chain = ListAttribute::from(Expressions::Immediate(vec![
        Immediate::DestinationRegister(Register::Verdict),
        Immediate::Data(DataAttribute::Verdict(vec![
            VerdictAttribute::Code(Verdict::Goto),
            VerdictAttribute::Chain(forwarding_chain.clone()),
        ])),
    ]));

    // Switch PathIP of packets
    // => ip6 daddr <PATH_IP_PREFIX> goto <FORWARDING_CHAIN>, e.g. fcaa::/16
    let mut hookin_rule_path_ip = NetlinkMessage::from(NetfilterMessage::new(
        NetfilterHeader::new(NetfilterProtoFamily::IPv6, 0, 0),
        NfTablesMessage::NewRule(RuleMessage {
            attributes: vec![
                RuleAttribute::Table(table.clone()),
                RuleAttribute::Chain(hookin_chain.clone()),
                RuleAttribute::Expressions({
                    let mut expressions = Vec::with_capacity(3);
                    expressions.extend(filter_16_bit_prefix(ip_prefixes.path_ip_prefix().bits()));
                    expressions.push(goto_forwarding_chain.clone());
                    expressions
                }),
            ],
        }),
    ));
    hookin_rule_path_ip.header.flags |= NLM_F_CREATE | NLM_F_APPEND;

    // Switch NodeIP of packets (but don't disturbe not icmpv6)
    // => ip6 daddr <NODE_IP_PREFIX> meta l4proto != icmpv6 goto <FORWARDING_CHAIN>, e.g. fc00::/16
    let mut hookin_rule_node_ip = NetlinkMessage::from(NetfilterMessage::new(
        NetfilterHeader::new(NetfilterProtoFamily::IPv6, 0, 0),
        NfTablesMessage::NewRule(RuleMessage {
            attributes: vec![
                RuleAttribute::Table(table.clone()),
                RuleAttribute::Chain(hookin_chain),
                RuleAttribute::Expressions({
                    let mut expressions = Vec::with_capacity(5);
                    expressions.extend(filter_16_bit_prefix(ip_prefixes.node_ip_prefix().bits()));
                    expressions.extend(vec![
                        // Don't forward ICMPv6
                        Expressions::Meta(vec![
                            Meta::Key(MetaKey::L4Proto),
                            Meta::DestinationRegister(Register::Reg1),
                        ])
                        .into(),
                        Expressions::Cmp(vec![
                            Cmp::SourceRegister(Register::Reg1),
                            Cmp::Op(Operator::NotEqual),
                            Cmp::Data(DataAttribute::Value(vec![ICMPv6_PROTO])),
                        ])
                        .into(),
                        goto_forwarding_chain,
                    ]);
                    expressions
                }),
            ],
        }),
    ));
    hookin_rule_node_ip.header.flags |= NLM_F_CREATE | NLM_F_APPEND;

    // => ip6 daddr set ip6 daddr map @<FORWARDING_MAP>
    let mut switch_dest_ip_rule = NetlinkMessage::from(NetfilterMessage::new(
        NetfilterHeader::new(NetfilterProtoFamily::IPv6, 0, 0),
        NfTablesMessage::NewRule(RuleMessage {
            attributes: vec![
                RuleAttribute::Table(table),
                RuleAttribute::Chain(forwarding_chain),
                RuleAttribute::Expressions(vec![
                    // Safe full destination IPv6 in Reg1
                    Expressions::Payload(vec![
                        Payload::DestinationRegister(Register::Reg1),
                        Payload::Base(1),    // Start at IPv6 header
                        Payload::Offset(24), // Start of dest-addr
                        Payload::Len(size_of::<Ipv6Addr>() as _),
                    ])
                    .into(),
                    // Safe new IPv6 in Reg1
                    Expressions::Lookup(vec![
                        Lookup::SourceRegister(Register::Reg1),
                        Lookup::DestinationRegister(Register::Reg1),
                        Lookup::Set(forwarding_map),
                        Lookup::SetId(1),
                    ])
                    .into(),
                    // Update destination IPv6 (and checksum)
                    Expressions::Payload(vec![
                        Payload::SourceRegister(Register::Reg1),
                        Payload::Base(1),    // Start at IPv6 header
                        Payload::Offset(24), // Start of dest-addr
                        Payload::Len(size_of::<Ipv6Addr>() as _),
                        Payload::CsumFlags(ChecksumFlags::L4PseudoHeader), /* update checksum */
                    ])
                    .into(),
                    ListAttribute::Element(vec![ExpressionAttribute::Name("notrack".to_string())]),
                ]),
            ],
        }),
    ));
    switch_dest_ip_rule.header.flags |= NLM_F_CREATE | NLM_F_APPEND;

    let batch_begin = NetlinkMessage::from(NetfilterMessage::new(
        NetfilterHeader::new(
            NetfilterProtoFamily::Unspec,
            0,
            u8::from(NetfilterSubsystem::NfTables) as _,
        ),
        ControlMessage::BatchBegin,
    ));
    let batch_end = NetlinkMessage::from(NetfilterMessage::new(
        NetfilterHeader::new(
            NetfilterProtoFamily::Unspec,
            0,
            u8::from(NetfilterSubsystem::NfTables) as _,
        ),
        ControlMessage::BatchEnd,
    ));

    let mut batch = [
        (batch_begin, ReplyExpectation::NotExpected),
        (create_new_table, ReplyExpectation::Expected),
        (create_hookin_chain, ReplyExpectation::Expected),
        (create_forwarding_chain, ReplyExpectation::Expected),
        (create_forwarding_map, ReplyExpectation::Expected),
        (hookin_rule_path_ip, ReplyExpectation::Expected),
        (hookin_rule_node_ip, ReplyExpectation::Expected),
        (switch_dest_ip_rule, ReplyExpectation::Expected),
        (batch_end, ReplyExpectation::NotExpected),
    ];
    batch
        .iter_mut()
        .for_each(|(msg, _)| msg.header.flags |= NLM_F_REQUEST | NLM_F_ACK);
    // https://github.com/torvalds/linux/commit/bf2ac490d28c21a349e9eef81edc45320fca4a3c
    batch[0].0.header.flags ^= NLM_F_ACK;
    batch[batch.len() - 1].0.header.flags ^= NLM_F_ACK;

    let mut response =
        handle_nfnetlink.request_batch_with_expectation(batch, SocketAddr::new(0, 0))?;
    while let Some(response) = response.next().await {
        if let NetlinkPayload::Error(err) = response.payload {
            return Err(err.into());
        }
    }

    log::debug!(
        target: "native_fwd_table::netlink",
        "KIRA's NfTables table created"
    );

    Ok(())
}
