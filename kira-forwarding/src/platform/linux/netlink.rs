//! Manage links, addresses and routes using Netlink.

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
    NLM_F_CREATE,
    NLM_F_EXCL,
    NLM_F_REPLACE,
    NLM_F_REQUEST,
    NetlinkHeader,
    NetlinkMessage,
    NetlinkPayload,
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
    sys::SocketAddr,
};

use crate::{
    domain::{
        InterfaceId,
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
/// Additionally a Nftables component is required for realizing the Nftables rules
/// that manage the KIRA gre encapsulation interface.
pub struct ForwardingRtNetlink {
    handle: ConnectionHandle<RouteNetlinkMessage>,
    kira: InterfaceId,
}

/// Default name for the [encap interface](create_encap_interface).
pub const KIRA_INTERFACE_NAME: &str = "kira";

#[derive(Debug, Display, Error, From)]
/// Error type used in [ForwardingRtNetlink].
pub enum ForwardingRtNetlinkError {
    /// Failed to deliver the netlink request.
    #[display("Delivering the netlink request failed: {_0}")]
    NetlinkRequestDeliveryFailure(netlink_proto::Error<RouteNetlinkMessage>),
    #[display("Received netlink error as response: {_0:?}")]
    /// The netlink socket responded with an [ErrorMessage] to the request.
    NetlinkResponseError(#[error(ignore)] ErrorMessage),
}

/// Result used by [ForwardingRtNetlink].
pub type Result<T> = std::result::Result<T, ForwardingRtNetlinkError>;

impl ForwardingRtNetlink {
    /// Creates a new [ForwardingRtNetlink] instance with the [default name](KIRA_INTERFACE_NAME) for the encap interface.
    pub async fn new(handle: ConnectionHandle<RouteNetlinkMessage>) -> Result<Self> {
        let kira = create_encap_interface(&handle, KIRA_INTERFACE_NAME).await?;
        Ok(Self::with_kira_interface(handle, kira))
    }

    /// Create a new [ForwardingRtNetlink] instance.
    ///
    /// You must ensure that the supplied `kira` [InterfaceId] points to
    /// a valid encap interface. You can create one using [create_encap_interface].
    pub fn with_kira_interface(
        handle: ConnectionHandle<RouteNetlinkMessage>,
        kira: InterfaceId,
    ) -> Self {
        Self { handle, kira }
    }

    /// Encapsulates packets with a destination ip of `node_id`
    /// using the encapsulation device `kira` and sets the new outer destination to `path_id`.
    ///
    /// This rule is used for realizing an [NodeIdEncapsulationEntry].
    /// An existing rule is overwritten.
    ///
    /// [NodeIdEncapsulationEntry]: crate::domain::NodeIdEncapsulationEntry
    #[tracing::instrument(level = "trace", target = "native_fwd_table::netlink", skip_all, fields(%node_id, out_path_id=%path_id))]
    pub async fn replace_encap_route(
        &mut self,
        node_id: &NodeIdSubnet,
        path_id: &PathId,
    ) -> Result<()> {
        let (node_ip, prefix_length) = node_id.to_ipv6_subnet();

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
                RouteIp6Tunnel::Destination(Ipv6Addr::from(path_id)),
            )]));

        let msg = NetlinkMessage::new(nl_hdr, RouteNetlinkMessage::NewRoute(rt_msg).into());

        let mut response = self.handle.request(msg, SocketAddr::new(0, 0))?;

        while let Some(response) = response.next().await {
            if let NetlinkPayload::Error(err) = response.payload {
                return Err(err.into());
            }
        }

        log::debug!(target: "native_fwd_table::netlink", "encap route replaced: {:?} encap {:?} ", node_id, path_id);

        Ok(())
    }

    /// Stops encapsulating packets with destination `node_id`.
    #[tracing::instrument(level = "trace", target = "native_fwd_table::netlink", skip_all, fields(%node_id, out_path_id=%path_id))]
    pub async fn delete_encap_route(
        &mut self,
        node_id: &NodeIdSubnet,
        path_id: &PathId,
    ) -> Result<()> {
        let (node_ip, prefix_length) = node_id.to_ipv6_subnet();

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
                RouteIp6Tunnel::Destination(Ipv6Addr::from(path_id)),
            )]));

        let msg = NetlinkMessage::new(nl_hdr, RouteNetlinkMessage::DelRoute(rt_msg).into());

        let mut response = self.handle.request(msg, SocketAddr::new(0, 0))?;

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
    /// [forwarding_rule]: crate::platform::add_forwarding_rule
    #[tracing::instrument(level = "trace", target = "native_fwd_table::netlink", skip_all, fields(%path_id, ?next_hop))]
    // TODO: figure out where route should get deleted
    pub async fn replace_via_route(
        &mut self,
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
            RouteAttribute::Destination(RouteAddress::Inet6(path_id.into())),
            RouteAttribute::Gateway(RouteAddress::Inet6(next_hop.ll_ipv6)),
            RouteAttribute::Oif(next_hop.interface_id.into()),
        ];

        let msg = NetlinkMessage::new(nl_hdr, RouteNetlinkMessage::NewRoute(rt_msg).into());

        let mut response = self.handle.request(msg, SocketAddr::new(0, 0))?;

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
    #[tracing::instrument(level = "trace", target = "native_fwd_table::netlink", skip_all, fields(%node_id, interface=%interface_id))]
    pub async fn replace_neighbor_route(
        &mut self,
        node_id: &NodeIdSubnet,
        interface_id: InterfaceId,
    ) -> Result<()> {
        let (node_ip, prefix_length) = node_id.to_ipv6_subnet();

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

        let mut response = self.handle.request(msg, SocketAddr::new(0, 0))?;

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
    #[tracing::instrument(level = "trace", target = "native_fwd_table::netlink", skip_all, fields(%node_id, interface=%interface_id))]
    pub async fn delete_neighbor_route(
        &mut self,
        node_id: &NodeIdSubnet,
        interface_id: InterfaceId,
    ) -> Result<()> {
        let (node_ip, prefix_length) = node_id.to_ipv6_subnet();

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

        let mut response = self.handle.request(msg, SocketAddr::new(0, 0))?;

        while let Some(response) = response.next().await {
            if let NetlinkPayload::Error(err) = response.payload {
                return Err(err.into());
            }
        }

        log::debug!(target: "native_fwd_table::netlink", "route deleted: {} dev {}", node_id, interface_id);

        Ok(())
    }

    /// Attaches the corresponding IPv6 address of `node_id` to the interface with name `interface`.
    ///
    /// The `node_id` usually is the root-id of KIRA instance running on the node.
    #[tracing::instrument(level = "trace", target = "native_fwd_table::netlink")]
    pub async fn attach_node_id_ip(
        &mut self,
        node_id: &NodeId,
        interface_id: InterfaceId,
    ) -> Result<()> {
        let node_id = node_id.into();

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
            .push(AddressAttribute::Address(std::net::IpAddr::V6(node_id)));

        let msg = NetlinkMessage::new(nl_hdr, RouteNetlinkMessage::NewAddress(rt_msg).into());

        let mut response = self.handle.request(msg, SocketAddr::new(0, 0))?;

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

        let mut response = self.handle.request(msg, SocketAddr::new(0, 0))?;

        while let Some(response) = response.next().await {
            if let NetlinkPayload::Error(err) = response.payload {
                return Err(err.into());
            }
        }

        log::debug!(target: "native_fwd_table::netlink", "deleted encap interface: {:?}", self.kira);

        Ok(())
    }
}

/// Creates an interface for encapsulating and decapsulating
/// IPv6 packets using [GRE](https://datatracker.ietf.org/doc/rfc7676/).
///
/// This interface is fully managed by the Nftables component.
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
