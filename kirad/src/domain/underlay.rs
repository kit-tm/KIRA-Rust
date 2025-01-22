use std::{collections::HashSet, net::Ipv6Addr};

pub use kira_lib::domain::{InterfaceId, UnderlayNeighborId, UnderlayNeighborUpdate};

pub type EthAddr = [u8; 6];

#[derive(Debug, Clone)]
pub struct Interface {
    pub idx: InterfaceId,
    pub src_mac: EthAddr,
    pub broadcast_mac: EthAddr,
    neighbors: HashSet<UnderlayNeighborId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UnderlayNeighbor {
    pub ll_ipv6: Ipv6Addr,
    pub if_index: InterfaceId,
}
#[derive(Debug, Clone)]
pub struct UnderlayNeighborInformation {
    pub if_index: InterfaceId,
    pub src_mac: EthAddr,
    pub broadcast_mac: EthAddr,
    pub ll_ipv6: Ipv6Addr,
}

impl UnderlayNeighbor {
    pub fn new(ll_ipv6: Ipv6Addr, interface: InterfaceId) -> Self {
        assert!(
            ll_ipv6.is_unicast_link_local(),
            "IPv6 should be link-local unicast"
        );

        Self {
            ll_ipv6,
            if_index: interface,
        }
    }
}

impl Interface {
    pub fn new(if_index: InterfaceId, src_mac: EthAddr, broadcast_mac: EthAddr) -> Self {
        Self {
            idx: if_index,
            src_mac,
            broadcast_mac,
            // one neighbor per link should be the norm
            // e.g.: Containernet simulation == 1
            neighbors: HashSet::with_capacity(1),
        }
    }

    pub fn add_neighbor(&mut self, ulnid: UnderlayNeighborId) {
        assert!(
            self.neighbors.insert(ulnid),
            "UnderlayNeighbors can only be registered once"
        );
    }

    pub fn neighbors(&self) -> impl Iterator<Item = &UnderlayNeighborId> {
        self.neighbors.iter()
    }

    pub fn into_neighbors(self) -> impl Iterator<Item = UnderlayNeighborId> {
        self.neighbors.into_iter()
    }
}

impl UnderlayNeighborInformation {
    pub fn new(neighbor: &UnderlayNeighbor, interface: &Interface) -> Self {
        assert_eq!(
            neighbor.if_index, interface.idx,
            "Interface of underlay neighbor should match supplied interface"
        );

        Self {
            if_index: neighbor.if_index,
            src_mac: interface.src_mac,
            broadcast_mac: interface.broadcast_mac,
            ll_ipv6: neighbor.ll_ipv6,
        }
    }
}
