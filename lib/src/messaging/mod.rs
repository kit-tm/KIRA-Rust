//! Type definitions containing everything related to protocol message transmission.
//!
//! This includes message formatting through [ProtocolMessageFormat](format::ProtocolMessageFormat), the traits [ProtocolMessageSender] and [ProtocolMessageReceiver], as well as the [ProtocolMessage] enumeration containing all protocol message types.

use std::collections::HashMap;
use std::net::{SocketAddr, SocketAddrV6};
use std::sync::Arc;

use tokio::sync::RwLock;

#[cfg(feature = "in-memory-message-channel")]
pub use in_memory_message_channel::*;
pub use messages::*;
pub use receiver::*;
pub use sender::*;

use crate::domain::{NetworkInterface, NodeId};
#[cfg(feature = "pnet")]
pub use crate::pnet_interface_monitor::*;

#[cfg(feature = "serde")]
pub mod format;
#[cfg(feature = "in-memory-message-channel")]
pub mod in_memory_message_channel;
pub mod messages;
pub mod receiver;
pub mod sender;
pub mod source_route;
#[cfg(any(feature = "sync-wrapper", test))]
pub mod sync_wrapper;

/// Maps a node id to an IPv6 Address.
pub trait IpCache {
    /// Needs to return a clone due to synchronized access.
    fn get(&self, id: &NodeId) -> Option<SocketAddrV6>;
    fn insert(&self, id: NodeId, addr: SocketAddrV6) -> Option<SocketAddrV6>;
}

/// Maps a node id to an IPv6 Address.
#[async_trait::async_trait]
pub trait AsyncIpCache {
    /// Needs to return a clone due to synchronized access.
    async fn get(&self, id: &NodeId) -> Option<SocketAddrV6>;
    async fn insert(&self, id: NodeId, addr: SocketAddrV6) -> Option<SocketAddrV6>;
}

impl IpCache for Arc<RwLock<HashMap<NodeId, SocketAddrV6>>> {
    fn get(&self, id: &NodeId) -> Option<SocketAddrV6> {
        let lock = self.blocking_read();
        lock.get(id).cloned()
    }

    fn insert(&self, id: NodeId, addr: SocketAddrV6) -> Option<SocketAddrV6> {
        let mut lock = self.blocking_write();
        lock.insert(id, addr)
    }
}

#[async_trait::async_trait]
impl AsyncIpCache for Arc<RwLock<HashMap<NodeId, SocketAddrV6>>> {
    async fn get(&self, id: &NodeId) -> Option<SocketAddrV6> {
        let lock = self.read().await;
        lock.get(id).cloned()
    }

    async fn insert(&self, id: NodeId, addr: SocketAddrV6) -> Option<SocketAddrV6> {
        let mut lock = self.write().await;
        lock.insert(id, addr)
    }
}

/// Maps [SocketAddr] to [NetworkInterface]s.
///
/// Returns [None] if no mapping to an interface is present.
pub trait InterfaceMapper {
    /// Get the interface for a given address.
    fn get_interface(&self, input_addr: &SocketAddr) -> Option<NetworkInterface>;
}

/// Maps [SocketAddr] to [NetworkInterface]s.
///
/// Returns [None] if no mapping to a interface is present.
#[async_trait::async_trait]
pub trait AsyncInterfaceMapper {
    /// Get the interface for a given address.
    async fn get_interface(&self, input_addr: &SocketAddr) -> Option<NetworkInterface>;
}

#[cfg(feature = "udp-tokio")]
pub mod udp {
    use std::net::{Ipv6Addr, SocketAddr};
    use std::sync::Arc;

    use unix_udp_sock::UdpSocket;

    use crate::messaging::format::ProtocolMessageFormat;
    use crate::messaging::{receiver, sender, AsyncInterfaceMapper, AsyncIpCache};

    /// Creates a asynchronous I/O Channel consisting of one [sender::udp_tokio::UdpSender] and
    /// one [receiver::udp_tokio::UdpReceiver] with UDP implementations.
    ///
    /// The [sender::udp_tokio::UdpSender] and [receiver::udp_tokio::UdpReceiver] share the same
    /// [tokio::net::UdpSocket].
    /// This way multiple senders can send and multiple receivers can receive from the
    /// same [tokio::net::UdpSocket].
    /// But all [ProtocolMessages](crate::messaging::messages::ProtocolMessage) will only arrive
    /// at one receiver at the time.
    pub async fn async_channel<C, P>(
        port: u16,
        cache: C,
        interface_mapper: P,
        format: ProtocolMessageFormat,
    ) -> tokio::io::Result<(
        sender::udp_tokio::UdpSender<C>,
        receiver::udp_tokio::UdpReceiver<C, P>,
    )>
    where
        C: AsyncIpCache + Clone + Send + Sync,
        P: AsyncInterfaceMapper + Clone + Send + Sync,
    {
        let udp_socket =
            UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], port))).await?;

        if let Err(err) =
            udp_socket.join_multicast_v6(&Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1), 0).await
        {
            log::trace!("Error joining multicast group: {:?}", err);
        }

        let socket = Arc::new(udp_socket);

        let sender = sender::udp_tokio::UdpSender::from_socket(
            socket.clone(),
            cache.clone(),
            format.clone(),
        )
        .await?;

        let receiver =
            receiver::udp_tokio::UdpReceiver::from_socket(socket, format, cache, interface_mapper);

        Ok((sender, receiver))
    }
}
