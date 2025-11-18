//! Definitions for KIRA to interact with the underlay network.
//!
//! This information is usually collected by an
//! [UnderlayObserverConnection](crate::underlay::UnderlayObserverConnection)
//! and accessed using an [UnderlayObserverHandle](crate::underlay::UnderlayObserverHandle).
//! The data structure holding the information is the
//! [UnderlayInformationBase](crate::underlay::information_base::UnderlayInformationBase)

use std::net::Ipv6Addr;

use kira_forwarding::underlay::UnderlayNeighborInformation;
pub use kira_r2kad::domain::{
    ConnectionId, InterfaceId, UnderlayNeighborId, UnderlayNeighborUpdate,
};

/// Ethernet Address.
pub type EthAddr = [u8; 6]; // should probably be a newtype on best practice

/// A struct storing all necessary information on an network interface
/// for the KIRA daemon.
///
/// This information is used through the [UnderlayNeighborInformation] by
/// the fast forwarding layer and the [io-part](crate::io) of R²/KAD.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    interface_id: InterfaceId,
    src_mac: EthAddr,
    broadcast_mac: EthAddr,
}

impl Interface {
    /// Creates a new [Interface]
    pub const fn new(if_index: InterfaceId, src_mac: EthAddr, broadcast_mac: EthAddr) -> Self {
        // TODO: do some sanity checks on EthAddr

        Self {
            interface_id: if_index,
            src_mac,
            broadcast_mac,
        }
    }

    /// Id of the [Interface].
    pub fn interface_id(&self) -> &InterfaceId {
        &self.interface_id
    }

    /// Ethernet address used for sending messages from this [Interface].
    pub fn src_mac(&self) -> &EthAddr {
        &self.src_mac
    }

    /// Ethernet address used for broadcasting messages from this [Interface].
    pub fn broadcast_mac(&self) -> &EthAddr {
        &self.broadcast_mac
    }
}

/// An struct containing all information about an underlay neighbor
/// excluding the actual [Interface] data.
///
/// An underlay neighbor is discovered by the [io-part](crate::io) when receiving
/// R²/KAD [ProtocolMessages](kira_r2kad::messaging::ProtocolMessage) from
/// unknown underlay neighbor sources.
///
/// This information is used through the [UnderlayNeighborInformation] by
/// the fast forwarding layer and the [io-part](crate::io) of R²/KAD.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UnderlayNeighbor {
    ll_ipv6: Ipv6Addr,
    interface_id: InterfaceId,
}

impl UnderlayNeighbor {
    /// Creates a new [UnderlayNeighbor].
    ///
    /// If the `ll_ipv6` is not a link-local unicast address this method will panic.
    pub fn new(ll_ipv6: Ipv6Addr, interface: InterfaceId) -> Self {
        assert!(
            ll_ipv6.is_unicast_link_local(),
            "IPv6 should be link-local unicast"
        );

        Self {
            ll_ipv6,
            interface_id: interface,
        }
    }

    /// Collects the information about an [UnderlayNeighbor]
    /// from the [UnderlayNeighbor] and the [Interface] under which it can be reached.
    ///
    /// If the `interface id`s don't match the method will panic.
    pub fn information(&self, interface: &Interface) -> UnderlayNeighborInformation {
        assert_eq!(
            self.interface_id, interface.interface_id,
            "Interface of underlay neighbor should match supplied interface"
        );

        UnderlayNeighborInformation {
            interface_id: self.interface_id,
            src_mac: interface.src_mac,
            broadcast_mac: interface.broadcast_mac,
            ll_ipv6: self.ll_ipv6,
        }
    }

    /// Link-local IPv6 address under which the neighbor can be reached.
    pub fn ll_ipv6(&self) -> &Ipv6Addr {
        &self.ll_ipv6
    }

    /// Id of the interface under which the neighbor can be reached.
    pub fn interface_id(&self) -> &InterfaceId {
        &self.interface_id
    }
}
