//! Interaction with the underlay of the node instance.

pub mod connection;
pub mod handle;

pub use connection::UnderlayObserverConnection;
pub use handle::UnderlayObserverHandle;

use std::collections::{HashMap, HashSet};
use std::net::Ipv6Addr;
use std::num::NonZeroUsize;

use derive_more::derive::{Display, Error};
use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};

use netlink_proto::new_connection;
use netlink_proto::sys::protocols::NETLINK_ROUTE;

use crate::domain::underlay::{
    Interface, InterfaceId, UnderlayNeighbor, UnderlayNeighborId, UnderlayNeighborInformation,
    UnderlayNeighborUpdate,
};

/// Sender of [UnderlayNeighborUpdates](UnderlayNeighborUpdate).
pub type UnderlayNeighborUpdatesTx = UnboundedSender<UnderlayNeighborUpdate>;
/// Receiver of [UnderlayNeighborUpdates](UnderlayNeighborUpdate).
///
/// The receiver is returned on [observe_underlay].
pub type UnderlayNeighborUpdatesRx = UnboundedReceiver<UnderlayNeighborUpdate>;

#[derive(Debug, Display, Clone, Error)]
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
        let neighbor = self.neighbors.get(ulnid)?;
        let interface = self
            .interfaces
            .get(&neighbor.interface_id)
            .expect("interface should be up as enforced by register_neighbor");

        let info = UnderlayNeighborInformation::new(neighbor, interface);
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

        // interface of the underlay neighbor
        let interface = self
            .interfaces
            .get_mut(&interface_id)
            .ok_or(UnderlayNeighborInterfaceDownError(interface_id))?;

        // FIXME: Handle gracefully
        assert!(
            self.neighbors.insert(self.next_ulnid, neighbor).is_none(),
            "Collision of life underlay neighbor ids because of overflow"
        );
        debug_assert!(self
            .neighbor_ids
            .insert(neighbor, self.next_ulnid)
            .is_none());

        // update interface with new neighbor
        interface.add_neighbor(self.next_ulnid);

        log::debug!(
            target: "underlay_observer::information_base",
            "underlay neighbor registered: {:?} -> {:?}",
            self.next_ulnid,
            neighbor,
        );

        // increment underlay neighbor id
        let UnderlayNeighborId(ulnid) = self.next_ulnid;
        // NOTE: overflow can occur here but this is only a problem on "live" underlay neighbor ids
        self.next_ulnid = UnderlayNeighborId(ulnid.checked_add(1).unwrap_or(NonZeroUsize::MIN));
        Ok(self.next_ulnid)
    }

    pub fn unregister_neighbor(&mut self, ulnid: &UnderlayNeighborId) -> Option<UnderlayNeighbor> {
        log::trace!(target: "underlay_observer::information_base", "Unregistering neighbor: {ulnid:?}");

        let neighbor = self.neighbors.remove(ulnid)?;
        debug_assert_eq!(self.neighbor_ids.remove(&neighbor), Some(*ulnid));
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

    pub fn interface_up(&mut self, interface: Interface) {
        log::debug!(target: "underlay_observer::information_base", "Interface is up: {:?}", interface);
        assert!(
            self.interfaces
                .insert(interface.interface_id, interface)
                .is_none(),
            "Preexisting inteface with same index"
        );
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
/// use kirad::domain::underlay::{InterfaceId, UnderlayNeighborId};
/// use kirad::underlay::observe_underlay;
///
/// let (conn, handle, mut updates) = observe_underlay().unwrap();
///
/// tokio::spawn(conn);
///
/// tokio::spawn(async move {
///     sleep(Duration::from_secs(5)).await;
///     let ulnid = UnderlayNeighborId(0);
///     let loopback = InterfaceId::from(NonZeroUsize::new(1).unwrap());
///     let mac = Default::default();
///     let ip = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 42);
///
///     handle.register_neighbor(loopback, mac, ip).await.unwrap();
///
///     let information = handle.get_information(&ulnid).await.unwrap();
///     println!("Information on {ulnid:?}: {information:?}");
/// });
///
/// while let Some(update) = updates.next().await {
///     println!("Update: {update:?}");
/// }
/// ```
pub fn observe_underlay(
    excluded_interfaces: HashSet<InterfaceId>,
) -> std::io::Result<(
    UnderlayObserverConnection,
    UnderlayObserverHandle,
    UnderlayNeighborUpdatesRx,
)> {
    let (connection, handle, messages) = new_connection(NETLINK_ROUTE)?;
    let (updates_tx, updates_rx) = unbounded();
    let (handle_tx, handle_rx) = unbounded();

    let connection = UnderlayObserverConnection::new(
        excluded_interfaces,
        connection,
        handle,
        messages,
        updates_tx,
        handle_rx,
    )?;
    let handle = UnderlayObserverHandle::new(handle_tx);

    Ok((connection, handle, updates_rx))
}
