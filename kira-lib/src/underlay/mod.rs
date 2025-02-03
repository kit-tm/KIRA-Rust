//! Interaction with the underlay of the node instance.

pub mod connection;
pub mod handle;

pub use connection::UnderlayObserverConnection;
pub use handle::UnderlayObserverHandle;
use kira_forwarding::underlay::UnderlayNeighborInformation;
use netlink_packet_route::RouteNetlinkMessage;

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::net::Ipv6Addr;
use std::num::NonZeroUsize;

use derive_more::derive::{Display, Error};
use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};

use netlink_proto::sys::protocols::NETLINK_ROUTE;
use netlink_proto::{new_connection, ConnectionHandle};

use crate::domain::underlay::{
    Interface, InterfaceId, UnderlayNeighbor, UnderlayNeighborId, UnderlayNeighborUpdate,
};

/// Sender of [UnderlayNeighborUpdates](UnderlayNeighborUpdate).
pub type UnderlayNeighborUpdatesTx = UnboundedSender<UnderlayNeighborUpdate>;
/// Receiver of [UnderlayNeighborUpdates](UnderlayNeighborUpdate).
///
/// The receiver is returned on [observe_underlay].
pub type UnderlayNeighborUpdatesRx = UnboundedReceiver<UnderlayNeighborUpdate>;

#[derive(Debug, Display, Clone, Error, PartialEq, Eq)]
#[display("Interface {_0} that is used by the underlay neighbor is down")]
/// Interface of [UnderlayNeighbor] identified by the [InterfaceId] is down.
pub struct UnderlayNeighborInterfaceDownError(#[error(ignore)] pub InterfaceId);

/// Manages all [underlay data](crate::domain::underlay).
///
/// This struct is usually accessed by message parsing using the [UnderlayObserverHandle].
#[derive(Debug)]
pub struct UnderlayInformationBase {
    next_ulnid: UnderlayNeighborId,
    neighbors: HashMap<UnderlayNeighborId, UnderlayNeighbor>,
    neighbor_ids: HashMap<UnderlayNeighbor, UnderlayNeighborId>,
    interfaces: HashMap<InterfaceId, Interface>,
}

impl Default for UnderlayInformationBase {
    fn default() -> Self {
        Self {
            next_ulnid: NonZeroUsize::new(1).unwrap().into(),
            neighbors: Default::default(),
            neighbor_ids: Default::default(),
            interfaces: Default::default(),
        }
    }
}

impl UnderlayInformationBase {
    #![allow(missing_docs)]

    pub fn get_information(
        &self,
        ulnid: &UnderlayNeighborId,
    ) -> Option<UnderlayNeighborInformation> {
        let Some(neighbor) = self.neighbors.get(ulnid) else {
            log::warn!(target: "underlay_observer::information_base", "neighbor id is unknown: {:?}", ulnid);
            return None;
        };
        let interface = self
            .interfaces
            .get(&neighbor.interface_id)
            .expect("interface should be up as enforced by register_neighbor");

        let info = neighbor.information(interface);
        log::trace!(target: "underlay_observer::information_base", "get_information {}: {:?}", ulnid, info);
        Some(info)
    }

    pub fn register_neighbor(
        &mut self,
        interface_id: InterfaceId,
        ll_ipv6: Ipv6Addr,
    ) -> Result<UnderlayNeighborId, UnderlayNeighborInterfaceDownError> {
        let neighbor = UnderlayNeighbor::new(ll_ipv6, interface_id);
        if let Some(ulnid) = self.neighbor_ids.get(&neighbor) {
            log::trace!(target: "underlay_observer", "Underlay Neighbor {neighbor:?} already registered: {ulnid:?}");
            return Ok(*ulnid);
        }

        let ulnid = self.next_ulnid;

        // interface of the underlay neighbor
        let interface = self
            .interfaces
            .get_mut(&interface_id)
            .ok_or(UnderlayNeighborInterfaceDownError(interface_id))?;

        // FIXME: Handle gracefully
        assert!(
            self.neighbors.insert(ulnid, neighbor).is_none(),
            "Collision of life underlay neighbor ids because of overflow"
        );
        let existing = self.neighbor_ids.insert(neighbor, ulnid);
        debug_assert!(existing.is_none());

        // update interface with new neighbor
        interface.add_neighbor(ulnid);

        log::debug!(
            target: "underlay_observer::information_base",
            "underlay neighbor registered: {:?} -> {:?}",
            self.next_ulnid,
            neighbor,
        );

        // increment underlay neighbor id
        {
            let UnderlayNeighborId(ulnid) = ulnid;
            // NOTE: overflow can occur here but this is only a problem on "live" underlay neighbor ids
            //       and checked via an assert
            self.next_ulnid = UnderlayNeighborId(ulnid.checked_add(1).unwrap_or(NonZeroUsize::MIN));
        }
        Ok(ulnid)
    }

    pub fn unregister_neighbor(&mut self, ulnid: &UnderlayNeighborId) -> Option<UnderlayNeighbor> {
        log::trace!(target: "underlay_observer::information_base", "Unregistering neighbor: {ulnid:?}");

        let neighbor = self.neighbors.remove(ulnid)?;
        let removed = self.neighbor_ids.remove(&neighbor);
        debug_assert_eq!(removed, Some(*ulnid));
        Some(neighbor)
    }

    pub fn interface_down(
        &mut self,
        interface_id: &InterfaceId,
    ) -> Option<impl Iterator<Item = UnderlayNeighborId>> {
        let interface = self.interfaces.remove(interface_id)?;

        log::debug!(target: "underlay_observer::information_base", "Downing all neighbors of interface: {}", interface_id);
        for ulnid in interface.neighbors() {
            let _ = self.unregister_neighbor(ulnid);
        }

        Some(interface.into_neighbors())
    }

    pub fn interface_up(&mut self, interface: Interface) -> bool {
        log::debug!(target: "underlay_observer::information_base", "Interface is up: {:?}", interface);
        match self.interfaces.entry(interface.interface_id) {
            Entry::Occupied(mut entry) => {
                if interface.src_mac != entry.get().src_mac
                    || interface.broadcast_mac != entry.get().broadcast_mac
                {
                    log::warn!(target: "underlay_observer::information_base", "Preexisting inteface with same index updated: {:?}", interface.interface_id);
                    // add old neighbors back
                    for ulnid in entry.insert(interface).into_neighbors() {
                        entry.get_mut().add_neighbor(ulnid)
                    }
                }
                false
            }
            Entry::Vacant(entry) => {
                entry.insert(interface);
                true
            }
        }
    }

    pub fn get_available(&self) -> impl Iterator<Item = &InterfaceId> {
        self.interfaces.keys()
    }
}

/// Create a new [UnderlayObserverConnection] and returns a [handle][UnderlayObserverHandle]
/// to that connection as well as a stream of [UnderlayNeighborUpdates][UnderlayNeighborUpdate].
///
/// If the netlink connection utilized by the [UnderlayObserverConnection] can't be established
/// successfully an io error is returned.
///
/// # Example
///
/// This example shows how to listen to all [UnderlayNeighborUpdates][UnderlayNeighborUpdate]
/// and register new [UnderlayNeighbors][UnderlayNeighbor] simultaneously.
///
/// ```rust,no_run
/// use std::net::Ipv6Addr;
/// use std::num::NonZeroUsize;
///
/// use futures::StreamExt;
/// use tokio::time::{sleep, Duration};
///
/// use kirad_lib::domain::underlay::{InterfaceId, UnderlayNeighborId};
/// use kirad_lib::underlay::observe_underlay;
///
/// #[tokio::main]
/// async fn main() {
///     let (conn, mut handle, mut updates, _) = observe_underlay(Default::default()).unwrap();
///
///     tokio::spawn(conn);
///
///     tokio::spawn(async move {
///         sleep(Duration::from_secs(5)).await;
///         // register underlay neighbor and get information back
///         let loopback = InterfaceId::try_from(1).unwrap();
///         let ip = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 42);
///
///         let ulnid = handle.register_neighbor(loopback, ip).await.unwrap();
///
///         let information = handle.get_information(&ulnid).await.unwrap();
///         println!("Information on {ulnid:?}: {information:?}");
///     });
///
///     while let Some(update) = updates.next().await {
///         println!("Update: {update:?}");
///     }
/// }
/// ```
pub fn observe_underlay(
    excluded_interfaces: HashSet<InterfaceId>,
) -> std::io::Result<(
    UnderlayObserverConnection,
    UnderlayObserverHandle,
    UnderlayNeighborUpdatesRx,
    ConnectionHandle<RouteNetlinkMessage>,
)> {
    let (connection, raw_handle, messages) = new_connection(NETLINK_ROUTE)?;
    let (updates_tx, updates_rx) = unbounded();
    let (handle_tx, handle_rx) = unbounded();

    let connection = UnderlayObserverConnection::new(
        excluded_interfaces,
        connection,
        raw_handle.clone(),
        messages,
        updates_tx,
        handle_rx,
    )?;
    let handle = UnderlayObserverHandle::new(handle_tx);

    Ok((connection, handle, updates_rx, raw_handle))
}

#[cfg(test)]
mod test {
    use super::*;

    const LL_IPV6: Ipv6Addr = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1);

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
    #[should_panic]
    fn avoid_ulnid_collision() {
        let mut ulnib = UnderlayInformationBase::default();

        // insert link with matching ID first
        let interface_id = InterfaceId::try_from(42).unwrap();
        let interface = Interface::new(interface_id, [0; 6], [0; 6]);
        ulnib.interfaces.insert(interface_id, interface);

        // set next id to live id
        let live_id = UnderlayNeighborId(42.try_into().unwrap());
        ulnib.next_ulnid = live_id;

        ulnib.neighbors.insert(
            live_id,
            UnderlayNeighbor {
                ll_ipv6: LL_IPV6,
                interface_id: InterfaceId::try_from(42).unwrap(),
            },
        );
        let _ = ulnib.register_neighbor(interface_id, LL_IPV6);
    }

    #[test]
    fn avoid_interface_down() {
        let mut ulnib = UnderlayInformationBase::default();

        let down_interface = InterfaceId::try_from(42).unwrap();
        assert_eq!(
            ulnib.register_neighbor(down_interface, LL_IPV6),
            Err(UnderlayNeighborInterfaceDownError(down_interface))
        );
    }

    #[test]
    fn correct_next_ulnid() {
        let mut ulnib = UnderlayInformationBase::default();

        // insert link with matching ID first
        let interface_id = InterfaceId::try_from(42).unwrap();
        let interface = Interface::new(interface_id, [0; 6], [0; 6]);
        ulnib.interfaces.insert(interface_id, interface);

        // set next id to live id
        let expected_ulnid = ulnib.next_ulnid;
        let ulnid = ulnib.register_neighbor(interface_id, LL_IPV6).unwrap();
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
