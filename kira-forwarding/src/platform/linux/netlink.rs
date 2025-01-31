//! Manage links, addresses and routes using Netlink.

use derive_more::derive::{Display, Error, From};
use futures::StreamExt;
use kira_lib::domain::NodeIdSubnet;
use std::net::Ipv6Addr;
use std::num::NonZeroU32;

use netlink_packet_core::{
    ErrorMessage, NetlinkHeader, NetlinkMessage, NetlinkPayload, NLM_F_ACK, NLM_F_CREATE,
    NLM_F_EXCL, NLM_F_REPLACE, NLM_F_REQUEST,
};
use netlink_packet_route::address::{AddressAttribute, AddressHeader, AddressMessage};
use netlink_packet_route::link::{
    AfSpecInet6, AfSpecUnspec, InfoData, InfoGreTun6, InfoKind, LinkAttribute, LinkFlags,
    LinkHeader, LinkInfo, LinkMessage,
};
use netlink_packet_route::route::{
    RouteAddress, RouteAttribute, RouteHeader, RouteLwEnCapType, RouteLwTunnelEncap, RouteMessage,
    RouteProtocol,
};
use netlink_packet_route::{AddressFamily, RouteNetlinkMessage};
use netlink_packet_utils::nla::DefaultNla;
use netlink_proto::{sys::SocketAddr, ConnectionHandle};

use crate::domain::{InterfaceId, NodeId, PathId};
use crate::underlay::UnderlayNeighborInformation;

// https://github.com/torvalds/linux/blob/05dbaf8dd8bf537d4b4eb3115ab42a5fb40ff1f5/include/uapi/linux/lwtunnel.h#L39
// TODO: upstream bindings to netlink_packet_route
#[repr(u16)]
enum LwtunnelIp6 {
    Dst = 2,
}

// https://github.com/torvalds/linux/blob/05dbaf8dd8bf537d4b4eb3115ab42a5fb40ff1f5/include/uapi/linux/if_link.h#L456
#[repr(u8)]
enum AddrGenMode {
    None = 1,
}
// https://github.com/torvalds/linux/blob/72deda0abee6e705ae71a93f69f55e33be5bca5c/include/uapi/linux/if_tunnel.h#L78
const IFLA_GRE_COLLECT_METADATA: u16 = 18;

#[derive(Debug)]
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
    /// The netlink socket responed with an [ErrorMessage] to the request.
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
    /// This rule is used for realizing an [Encapsulate NodeIdEntry](crate::tables::NodeIdEntry::Encapsulate).
    /// An existing rule is overwritten.
    #[tracing::instrument(level = "trace", target = "native_fwd_tables::netlink")]
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
            .push(RouteAttribute::Encap(vec![RouteLwTunnelEncap::Other(
                DefaultNla::new(
                    LwtunnelIp6::Dst as u16,
                    Ipv6Addr::from(path_id).octets().to_vec(),
                ),
            )]));

        let msg = NetlinkMessage::new(nl_hdr, RouteNetlinkMessage::NewRoute(rt_msg).into());

        let mut response = self.handle.request(msg, SocketAddr::new(0, 0))?;

        while let Some(response) = response.next().await {
            if let NetlinkPayload::Error(err) = response.payload {
                return Err(err.into());
            }
        }

        log::debug!(target: "native_fwd_tables::netlink", "encap route replaced: {:?} encap {:?} ", node_id, path_id);

        Ok(())
    }

    /// Stops encapsulating packets with destination `node_id`.
    #[tracing::instrument(level = "trace", target = "native_fwd_tables::netlink")]
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
            .push(RouteAttribute::Encap(vec![RouteLwTunnelEncap::Other(
                DefaultNla::new(
                    LwtunnelIp6::Dst as u16,
                    Ipv6Addr::from(path_id).octets().to_vec(),
                ),
            )]));

        let msg = NetlinkMessage::new(nl_hdr, RouteNetlinkMessage::DelRoute(rt_msg).into());

        let mut response = self.handle.request(msg, SocketAddr::new(0, 0))?;

        while let Some(response) = response.next().await {
            if let NetlinkPayload::Error(err) = response.payload {
                return Err(err.into());
            }
        }

        log::debug!(target: "native_fwd_tables::netlink", "encap route deleted: {:?} encap {:?} ", node_id, path_id);

        Ok(())
    }

    /// Forwards packets with destination `path_ip` as if they where destined to `next_hop_ip`.
    ///
    /// This rule is used to determine to which underlay neighbor (`next_hop_ip`)
    /// a packet with a given [PathId] is forwarded.
    /// This rule is used in combination with a [forwarding_rule](add_forwarding_rule)
    /// to realize A [Forward PathIdEntry](crate::tables::PathIdEntry::Forward).
    /// An existing rule is overwritten.

    #[tracing::instrument(level = "trace", target = "native_fwd_tables::netlink")]
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

        log::debug!(target: "native_fwd_tables::netlink", "route replaced: {:?} via {}%{}", path_id, next_hop.ll_ipv6, next_hop.interface_id);

        Ok(())
    }

    /// Forwards packets with destination `ip` to interface with name `interface_name` unchanged.
    ///
    /// This is used to realize [Forward NodeIdEntry](crate::tables::NodeIdEntry::Forward).
    #[tracing::instrument(level = "trace", target = "native_fwd_tables::netlink")]
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

        log::debug!(target: "native_fwd_tables::netlink", "route replaced: {} dev {}", node_ip, interface_id);

        Ok(())
    }

    /// Deletes a [neighbor route](replace_neighbor_route).
    #[tracing::instrument]
    pub async fn delete_neighbor_route(
        &mut self,
        ip: &Ipv6Addr,
        prefix: u8,
        interface_id: InterfaceId,
    ) -> Result<()> {
        let mut nl_hdr = NetlinkHeader::default();
        nl_hdr.flags = NLM_F_REQUEST | NLM_F_ACK;

        let mut rt_msg = RouteMessage::default();
        rt_msg.header = RouteHeader {
            address_family: AddressFamily::Inet6,
            destination_prefix_length: prefix,
            protocol: RouteProtocol::Static,
            ..rt_msg.header
        };
        rt_msg
            .attributes
            .push(RouteAttribute::Destination(RouteAddress::Inet6(*ip)));
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

        log::debug!(target: "native_fwd_tables::netlink", "route deleted: {} dev {}", ip, interface_id);

        Ok(())
    }

    /// Attaches the corresponding IPv6 address of `node_id` to the interface with name `interface`.
    ///
    /// The `node_id` usually is the root-id of KIRA instance running on the node.
    #[tracing::instrument(level = "trace", target = "native_fwd_tables::netlink")]
    pub async fn attach_node_id_ip(
        &mut self,
        node_id: &NodeId,
        interface_id: InterfaceId,
    ) -> Result<()> {
        let node_id = node_id.into();

        let mut nl_hdr = NetlinkHeader::default();
        nl_hdr.flags = NLM_F_REQUEST | NLM_F_CREATE | NLM_F_EXCL | NLM_F_ACK;

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

        log::debug!(target: "native_fwd_tables::netlink", "added node-id ip to interface: {} dev {}", node_id, interface_id);

        Ok(())
    }

    /// Deletes the [`kira` interface](create_kira_interface).
    ///
    /// This method consumes the struct because other methods rely on the interface
    /// to be present.
    #[tracing::instrument(level = "trace", target = "native_fwd_tables::netlink")]
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

        log::debug!(target: "native_fwd_tables::netlink", "deleted encap interface: {:?}", self.kira);

        Ok(())
    }
}

/// Creates an interface for encapsulating and decapsulating
/// IPv6 packets using [GRE](https://datatracker.ietf.org/doc/rfc7676/).
///
/// This interface is fully managed by the Nftables component.
#[tracing::instrument(level = "trace", target = "native_fwd_tables::netlink")]
pub async fn create_encap_interface(
    handle: &ConnectionHandle<RouteNetlinkMessage>,
    if_name: &str,
) -> Result<InterfaceId> {
    // create interface
    log::trace!(target: "native_fwd_tables::netlink", "create encap interface {}", if_name);
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
                LinkInfo::Data(InfoData::GreTun6(vec![InfoGreTun6::Other(
                    DefaultNla::new(IFLA_GRE_COLLECT_METADATA, vec![]), // externally managed (by Nftables)
                )])),
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

    log::trace!(target: "native_fwd_tables::netlink", "obtain interface index of encap interface");
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
    log::trace!(target: "native_fwd_tables::netlink", "Obtained interface id of interface {}: {}", if_name, kira);

    log::trace!(target: "native_fwd_tables::netlink", "disable address generation mode on interface");
    // disable address generation mode on interface
    {
        let mut rt_msg = LinkMessage::default();
        rt_msg.header = LinkHeader {
            index: kira.into(),
            ..rt_msg.header
        };
        rt_msg.attributes = vec![LinkAttribute::AfSpecUnspec(vec![AfSpecUnspec::Inet6(
            vec![AfSpecInet6::AddrGenMode(AddrGenMode::None as u8)],
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

    log::debug!(target: "native_fwd_tables::netlink", "created encap interface {}: {:?}", if_name, kira);

    Ok(kira)
}
