//! Interaction with the underlay of the node instance.

pub mod connection;
pub mod handle;
pub mod information_base;

use std::collections::HashSet;

pub use connection::UnderlayObserverConnection;
use futures::channel::mpsc::{
    UnboundedReceiver,
    UnboundedSender,
    unbounded,
};
pub use handle::UnderlayObserverHandle;
use netlink_packet_route::RouteNetlinkMessage;
use netlink_proto::{
    ConnectionHandle,
    new_connection,
    sys::protocols::NETLINK_ROUTE,
};

use crate::domain::underlay::{
    InterfaceId,
    UnderlayNeighborUpdate,
};

/// Sender of [UnderlayNeighborUpdates](UnderlayNeighborUpdate).
pub type UnderlayNeighborUpdatesTx = UnboundedSender<UnderlayNeighborUpdate>;

/// Receiver of [UnderlayNeighborUpdates](UnderlayNeighborUpdate).
///
/// The receiver is returned on [observe_underlay].
pub type UnderlayNeighborUpdatesRx = UnboundedReceiver<UnderlayNeighborUpdate>;

/// Create a new [UnderlayObserverConnection] and returns a [handle][UnderlayObserverHandle]
/// to that connection as well as a stream of [UnderlayNeighborUpdates][UnderlayNeighborUpdate].
///
/// If the netlink connection utilized by the [UnderlayObserverConnection] can't be established
/// successfully an io error is returned.
///
/// # Example
///
/// This example shows how to listen to all [UnderlayNeighborUpdates][UnderlayNeighborUpdate]
/// and register new [UnderlayNeighbors](crate::domain::underlay::UnderlayNeighbor) simultaneously.
///
/// ```
/// use std::net::Ipv6Addr;
/// use std::num::NonZeroUsize;
///
/// use futures::StreamExt;
/// use tokio::time::{sleep, Duration, timeout};
///
/// use kira_lib::domain::underlay::{InterfaceId, UnderlayNeighbor, UnderlayNeighborId};
/// use kira_lib::underlay::observe_underlay;
///
/// #[tokio::main(flavor = "current_thread")]
/// async fn main() {
///     let (conn, mut handle, mut updates, _) = observe_underlay(Default::default()).unwrap();
///
///     // start listening on interface changes
///     // automatically populates UnderlayInformationBase with currently present interfaces
///     tokio::spawn(conn);
///
///     tokio::spawn(async move {
///         // wait for the connection to populate UnderlayInformationBase with
///         // existing interfaces.
///         sleep(Duration::from_secs(5)).await;
///
///         // register underlay neighbor
///         let loopback = InterfaceId::try_from(1).unwrap();
///         let ip = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 42);
///         let neighbor = UnderlayNeighbor::new(ip, loopback);
///
///         let ulnid = handle.register_neighbor(neighbor)
///             .await
///             .expect("loopback should have been upped");
///
///         // and get information back
///         let information = handle.get_information(&ulnid).await.unwrap();
///         println!("Information on {ulnid:?}: {information:?}");
///     });
///
///     // print updates for 10 seconds
///     let output_loop = async move {
///         while let Some(update) = updates.next().await {
///             println!("Update: {update:?}");
///         }
///     };
///     timeout(Duration::from_secs(10), output_loop).await;
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
