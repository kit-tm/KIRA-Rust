//! Definitions for KIRA to interact with the underlay network.
//!
//! This information is usually collected by an
//! [UnderlayObserverConnection](crate::underlay::UnderlayObserverConnection)
//! and accessed using an [UnderlayObserverHandle](crate::underlay::UnderlayObserverHandle).
//! The data structure holding the information is the
//! [UnderlayInformationBase](crate::underlay::UnderlayInformationBase)

use std::{collections::HashSet, net::Ipv6Addr};

pub use kira_lib::domain::{InterfaceId, UnderlayNeighborId, UnderlayNeighborUpdate};

/// Ethernet Address.
pub type EthAddr = [u8; 6];

/// A struct storing all necessary information on an network interface
/// for the KIRA daemon.
///
/// This information is used through the [UnderlayNeighborInformation] by
/// the fast forwarding layer and the [io-part](crate::io) of R²/KAD.
#[derive(Debug, Clone)]
pub struct Interface {
    /// Id of the [Interface].
    pub interface_id: InterfaceId,
    /// Ethernet address used for sending messages from this [Interface].
    pub src_mac: EthAddr,
    /// Ethernet address used for broadcasting messages from this [Interface].
    pub broadcast_mac: EthAddr,
    /// List of all neighbors connected via this [Interface].
    neighbors: HashSet<UnderlayNeighborId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// An struct containing all information about an underlay neighbor
/// excluding the actual [Interface] data.
///
/// An underlay neighbor is discovered by the [io-part](crate::io) when receiving
/// R²/KAD [ProtocolMessages](kira_lib::messaging::ProtocolMessage) from
/// unknown underlay neighbor sources.
///
/// This information is used through the [UnderlayNeighborInformation] by
/// the fast forwarding layer and the [io-part](crate::io) of R²/KAD.
pub struct UnderlayNeighbor {
    /// link-local IPv6 address under which the neighbor can be reached.
    pub ll_ipv6: Ipv6Addr,
    /// Id of the interface under which the neighbor can be reached.
    pub interface_id: InterfaceId,
}

#[derive(Debug, Clone)]
/// All information known by KIRA about an underlay neighbor.
///
/// This information is used by the fast forwarding layer and the [io-part](crate::io) of R²/KAD.
/// You can obtain this struct using [UnderlayObserverHandle::get_information](crate::underlay::UnderlayObserverHandle::get_information)
pub struct UnderlayNeighborInformation {
    /// Id of the [Interface] under which the neighbor can be reached.
    pub interface_id: InterfaceId,
    /// Ethernet address used for sending messages to the neighbor from this [Interface].
    pub src_mac: EthAddr,
    /// Ethernet address used for broadcasting messages from this [Interface].
    pub broadcast_mac: EthAddr,
    /// link-local IPv6 address under which the neighbor can be reached.
    pub ll_ipv6: Ipv6Addr,
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
}

impl Interface {
    /// Creates a new [Interface]
    ///
    /// The initial neighbor capacity is set to `1`.
    pub fn new(if_index: InterfaceId, src_mac: EthAddr, broadcast_mac: EthAddr) -> Self {
        Self {
            interface_id: if_index,
            src_mac,
            broadcast_mac,
            // one neighbor per link should be the norm
            // e.g.: Containernet simulation == 1
            neighbors: HashSet::with_capacity(1),
        }
    }

    /// Add a new underlay neighbor specified by its [UnderlayNeighborId] to an [Interface].
    ///
    /// If the interface already has the [UnderlayNeighborId] added this method will panic.
    pub fn add_neighbor(&mut self, ulnid: UnderlayNeighborId) {
        assert!(
            self.neighbors.insert(ulnid),
            "UnderlayNeighbors can only be registered once"
        );
    }

    /// Returns a list of all underlay neighbors connected via this [Interface].
    pub fn neighbors(&self) -> impl Iterator<Item = &UnderlayNeighborId> {
        self.neighbors.iter()
    }

    /// Converts the [Interface] into all underlay neighbors connected via itself.
    pub fn into_neighbors(self) -> impl Iterator<Item = UnderlayNeighborId> {
        self.neighbors.into_iter()
    }
}

impl UnderlayNeighborInformation {
    /// Collects the information about an [UnderlayNeighbor]
    /// from the [UnderlayNeighbor] and the [Interface] under which it can be reached.
    ///
    /// If the `interface id`s don't match the method will panic.
    pub fn new(neighbor: &UnderlayNeighbor, interface: &Interface) -> Self {
        assert_eq!(
            neighbor.interface_id, interface.interface_id,
            "Interface of underlay neighbor should match supplied interface"
        );

        Self {
            interface_id: neighbor.interface_id,
            src_mac: interface.src_mac,
            broadcast_mac: interface.broadcast_mac,
            ll_ipv6: neighbor.ll_ipv6,
        }
    }
}
