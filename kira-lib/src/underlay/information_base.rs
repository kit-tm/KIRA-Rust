//! State and data storage of information collected about underlay connections.
//!
//! The main struct is the [UnderlayInformationBase].

use std::collections::{
    HashMap,
    HashSet,
    hash_map::Entry,
};

use derive_more::derive::{
    Display,
    Error,
};
use kira_forwarding::underlay::UnderlayNeighborInformation;

pub use super::{
    connection::UnderlayObserverConnection,
    handle::UnderlayObserverHandle,
};
use crate::domain::underlay::{
    ConnectionId,
    Interface,
    InterfaceId,
    UnderlayNeighbor,
    UnderlayNeighborId,
};

/// Interface of [UnderlayNeighbor] identified by the [InterfaceId] is down.
#[derive(Debug, Display, Clone, Error, PartialEq, Eq)]
#[display("Interface {_0} that is used by the underlay neighbor is down")]
pub struct UnderlayNeighborInterfaceDownError(#[error(ignore)] pub InterfaceId);

#[derive(Debug)]
struct InterfaceState {
    interface: Interface,

    next_cid: ConnectionId,
    neighbors: HashSet<ConnectionId>,
}

impl InterfaceState {
    /// Returns a list of all underlay neighbors connected via this [Interface].
    pub fn neighbors(&self) -> impl Iterator<Item = UnderlayNeighborId> {
        self.neighbors
            .iter()
            .copied()
            .map(|connection_id| UnderlayNeighborId {
                interface_id: *self.interface.interface_id(),
                connection_id,
            })
    }

    /// Converts the [Interface] into all underlay neighbors connected via itself.
    pub fn into_neighbors(self) -> impl Iterator<Item = UnderlayNeighborId> {
        self.neighbors
            .into_iter()
            .map(move |connection_id| UnderlayNeighborId {
                interface_id: *self.interface.interface_id(),
                connection_id,
            })
    }
}

impl From<Interface> for InterfaceState {
    fn from(interface: Interface) -> Self {
        InterfaceState {
            interface,
            next_cid: ConnectionId(0),
            neighbors: HashSet::new(),
        }
    }
}

/// Manages all [underlay data](crate::domain::underlay).
///
/// This struct is usually accessed by message parsing using the [UnderlayObserverHandle].
#[derive(Debug, Default)]
pub struct UnderlayInformationBase {
    neighbors: HashMap<UnderlayNeighborId, UnderlayNeighbor>,
    neighbor_ids: HashMap<UnderlayNeighbor, UnderlayNeighborId>,
    interfaces: HashMap<InterfaceId, InterfaceState>,
}

impl UnderlayInformationBase {
    #![allow(missing_docs)]

    pub fn get_information(
        &self,
        ulnid: &UnderlayNeighborId,
    ) -> Option<UnderlayNeighborInformation> {
        let Some(neighbor) = self.neighbors.get(ulnid) else {
            log::warn!(target: "underlay_observer::information_base", "neighbor id is unknown: {ulnid:?}");
            return None;
        };
        let InterfaceState { interface, .. } = self
            .interfaces
            .get(neighbor.interface_id())
            .expect("interface should be up as enforced by register_neighbor");

        let info = neighbor.information(interface);
        log::trace!(target: "underlay_observer::information_base", "get_information {ulnid}: {info:?}");
        Some(info)
    }

    pub fn register_neighbor(
        &mut self,
        neighbor: UnderlayNeighbor,
    ) -> Result<UnderlayNeighborId, UnderlayNeighborInterfaceDownError> {
        if let Some(ulnid) = self.neighbor_ids.get(&neighbor) {
            log::trace!(target: "underlay_observer", "Underlay Neighbor {neighbor:?} already registered: {ulnid:?}");
            return Ok(*ulnid);
        }

        // interface of the underlay neighbor
        let interface_state = self
            .interfaces
            .get_mut(neighbor.interface_id())
            .ok_or(UnderlayNeighborInterfaceDownError(*neighbor.interface_id()))?;

        let ulnid = UnderlayNeighborId {
            interface_id: *neighbor.interface_id(),
            connection_id: interface_state.next_cid,
        };

        // FIXME: Handle ConnectionId overflow gracefully
        assert!(
            self.neighbors.insert(ulnid, neighbor).is_none(),
            "Collision of life underlay neighbor ids because unhandles overflow"
        );
        assert!(
            self.neighbor_ids.insert(neighbor, ulnid).is_none(),
            "Generation a new ConnectionID necessitates an unknown UnderlayNeighbor"
        );

        // update interface with new neighbor
        assert!(
            interface_state.neighbors.insert(interface_state.next_cid),
            "ConnectionId should be unused"
        );

        log::debug!(
            target: "underlay_observer::information_base",
            "underlay neighbor registered: {ulnid:?} -> {neighbor:?}",
        );

        // increment underlay neighbor id
        {
            // overflow can occur here but this is only a problem on "live" ConnectionIds
            //       and checked for on insert
            interface_state.next_cid =
                ConnectionId(interface_state.next_cid.0.checked_add(1).unwrap_or(1));
        }
        Ok(ulnid)
    }

    pub fn unregister_neighbor(&mut self, ulnid: &UnderlayNeighborId) -> Option<UnderlayNeighbor> {
        log::trace!(target: "underlay_observer::information_base", "Unregistering neighbor: {ulnid:?}");

        let neighbor = self.neighbors.remove(ulnid)?;
        assert_eq!(self.neighbor_ids.remove(&neighbor), Some(*ulnid));

        assert!(
            self.interfaces
                .get_mut(&ulnid.interface_id)
                .expect("Interface of Underlay should be up")
                .neighbors
                .remove(&ulnid.connection_id),
            "UnderlayNeighbor not registered to Interface"
        );

        Some(neighbor)
    }

    pub fn interface_down(
        &mut self,
        interface_id: &InterfaceId,
    ) -> Option<impl Iterator<Item = UnderlayNeighborId>> {
        let interface_state = self.interfaces.remove(interface_id)?;

        log::debug!(target: "underlay_observer::information_base", "Downing all neighbors of interface: {interface_id}");
        for ulnid in interface_state.neighbors() {
            let neighbor = self.neighbors.remove(&ulnid)?;
            assert_eq!(self.neighbor_ids.remove(&neighbor), Some(ulnid));

            log::trace!(target: "underlay_observer::information_base", "Unregistering neighbor: {ulnid:?}");
        }

        Some(interface_state.into_neighbors())
    }

    pub fn interface_up(&mut self, interface: Interface) -> bool {
        log::debug!(target: "underlay_observer::information_base", "Interface is up: {interface:?}");
        match self.interfaces.entry(*interface.interface_id()) {
            Entry::Occupied(mut entry) => {
                let InterfaceState {
                    interface: existing_interface,
                    ..
                } = entry.get_mut();

                assert_eq!(
                    interface.interface_id(),
                    existing_interface.interface_id(),
                    "InterfaceId HashMap key inconsistency"
                );

                if &interface != existing_interface {
                    *existing_interface = interface;
                    log::warn!(target: "underlay_observer::information_base", "Preexisting interface with same index updated: {:?}", existing_interface.interface_id());
                }
                false
            }
            Entry::Vacant(entry) => {
                entry.insert(InterfaceState::from(interface));
                true
            }
        }
    }

    pub fn get_available(&self) -> impl Iterator<Item = &InterfaceId> {
        self.interfaces.keys()
    }
}

#[cfg(test)]
mod test {

    use std::net::Ipv6Addr;

    use super::*;
    use crate::domain::underlay::EthAddr;

    const LL_IPV6: Ipv6Addr = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1);
    const MAC: EthAddr = [0x00, 0x00, 0x5E, 0x00, 0x53, 0x00];
    const BROADCAST_MAC: EthAddr = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff];

    fn sample_interface() -> (InterfaceId, Interface) {
        let interface_id = InterfaceId::try_from(42).unwrap();
        let interface = Interface::new(interface_id, MAC, BROADCAST_MAC);

        (interface_id, interface)
    }

    #[test]
    fn empty_default() {
        let ulnib = UnderlayInformationBase::default();
        assert!(
            ulnib.get_available().next().is_none(),
            "no available interfaces"
        );

        let UnderlayInformationBase {
            neighbors,
            neighbor_ids,
            interfaces,
            ..
        } = ulnib;
        assert!(neighbors.is_empty(), "no neighbors");
        assert!(neighbor_ids.is_empty(), "no ids");
        assert!(interfaces.is_empty(), "no interfaces");
    }

    #[test]
    #[should_panic(expected = "Collision of life underlay neighbor ids because unhandles overflow")]
    fn avoid_ulnid_collision() {
        let mut ulnib = UnderlayInformationBase::default();

        // insert link with matching ID first
        let (interface_id, interface) = sample_interface();
        assert!(ulnib.interface_up(interface), "interface wasn't up");

        // TODO: cause collision

        let _ = ulnib.register_neighbor(UnderlayNeighbor::new(LL_IPV6, interface_id));

        // force wrap around
        ulnib
            .interfaces
            .get_mut(&interface_id)
            .expect("should be state associated with upped interface")
            .next_cid = ConnectionId::from(0);

        let _ = ulnib.register_neighbor(UnderlayNeighbor::new(
            Ipv6Addr::from_bits(LL_IPV6.to_bits() + 1),
            interface_id,
        ));
    }

    #[test]
    fn avoid_interface_down() {
        let mut ulnib = UnderlayInformationBase::default();

        let down_interface = InterfaceId::try_from(42).unwrap();
        assert_eq!(
            ulnib.register_neighbor(UnderlayNeighbor::new(LL_IPV6, down_interface)),
            Err(UnderlayNeighborInterfaceDownError(down_interface)),
            "shouldn't succesfully insert on interface not previously upped"
        );
    }

    #[test]
    fn correct_next_ulnid() {
        let mut ulnib = UnderlayInformationBase::default();

        // insert link with matching ID first
        let (interface_id, interface) = sample_interface();
        assert!(ulnib.interface_up(interface), "interface wasn't up");

        let next_cid = ConnectionId::from(42);

        ulnib
            .interfaces
            .get_mut(&interface_id)
            .expect("should be state associated with upped interface")
            .next_cid = next_cid;

        let expected_ulnid = UnderlayNeighborId {
            interface_id,
            connection_id: next_cid,
        };

        // set next id to live id
        let ulnid = ulnib
            .register_neighbor(UnderlayNeighbor::new(LL_IPV6, interface_id))
            .unwrap();
        assert_eq!(ulnid, expected_ulnid, "ulnid should be next ulnid");
    }

    #[test]
    fn interface() {
        let mut ulnib = UnderlayInformationBase::default();
        let interface_id = InterfaceId::try_from(42).unwrap();
        let interface = Interface::new(interface_id, [0; 6], [0; 6]);

        assert!(
            ulnib.interface_down(&interface_id).is_none(),
            "interface does not exist prior"
        );

        // interface up
        assert!(ulnib.interface_up(interface), "is a fresh interface");
        assert_eq!(
            ulnib.get_available().next(),
            Some(&interface_id),
            "interface should be available on up"
        );
        assert_eq!(
            ulnib.get_available().next(),
            Some(&interface_id),
            "interface should be available on up"
        );

        // interface down
        let mut affected_neighbors = ulnib.interface_down(&interface_id).expect("exists prior");
        assert_eq!(affected_neighbors.next(), None, "no neighbor down");
    }
}
