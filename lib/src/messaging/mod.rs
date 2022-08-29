use std::collections::HashMap;
use std::net::{SocketAddr, SocketAddrV6};
use std::sync::Arc;

use tokio::sync::RwLock;

pub use in_memory_message_hub::*;
pub use messages::*;
#[cfg(feature = "pnet")]
pub use pnet_port_mapper::*;
pub use receiver::*;
pub use sender::*;

use crate::domain::{NodeId, Port};

#[cfg(feature = "serde")]
pub mod format;
pub mod in_memory_message_hub;
pub mod messages;
#[cfg(feature = "pnet")]
pub mod pnet_port_mapper;
pub mod receiver;
pub mod sender;
pub mod source_route;
#[cfg(feature = "sync-wrapper")]
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

/// Maps [SocketAddr] to [Port]s.
///
/// Returns [None] if no mapping to a port is present.
pub trait PortMapper {
    /// Get the port for a given address.
    fn get_port(&self, input_addr: &SocketAddr) -> Option<Port>;
}

/// Maps [SocketAddr] to [Port]s.
///
/// Returns [None] if no mapping to a port is present.
/// The caller then may use [Port::All] for HelloMessages to flood them to all ports.
#[async_trait::async_trait]
pub trait AsyncPortMapper {
    /// Get the port for a given address.
    async fn get_port(&self, input_addr: &SocketAddr) -> Option<Port>;
}

pub mod udp {
    use std::net::SocketAddr;
    use std::sync::Arc;

    use crate::messaging::format::ProtocolMessageFormat;
    use crate::messaging::{receiver, sender, AsyncIpCache, AsyncPortMapper, IpCache, PortMapper};

    /// Creates a synchronous I/O Channel consisting of one [sender::udp::UdpSender] and
    /// one [receiver::udp::UdpReceiver].
    ///
    /// The [sender::udp::UdpSender] and [receiver::udp::UdpReceiver] share the same
    /// [std::net::UdpSocket].
    /// This way multiple senders can send and multiple receivers can receive from the
    /// same [UdpSocket].
    /// But all [ProtocolMessage]s will only arrive at one receiver at the time.
    pub fn sync_channel<C, P>(
        port: u16,
        cache: C,
        port_mapper: P,
        format: ProtocolMessageFormat,
    ) -> std::io::Result<(sender::udp::UdpSender<C>, receiver::udp::UdpReceiver<C, P>)>
    where
        C: IpCache + Clone,
        P: PortMapper,
    {
        let socket = Arc::new(std::net::UdpSocket::bind(SocketAddr::from((
            [0, 0, 0, 0, 0, 0, 0, 0],
            port,
        )))?);

        let sender =
            sender::udp::UdpSender::from_socket(socket.clone(), cache.clone(), format.clone())?;

        let receiver = receiver::udp::UdpReceiver::from_socket(socket, cache, port_mapper, format);

        Ok((sender, receiver))
    }

    /// Creates a asynchronous I/O Channel consisting of one [sender::udp_tokio::UdpSender] and
    /// one [receiver::udp_tokio::UdpReceiver] with UDP implementations.
    ///
    /// The [sender::udp_tokio::UdpSender] and [receiver::udp_tokio::UdpReceiver] share the same
    /// [tokio::net::UdpSocket].
    /// This way multiple senders can send and multiple receivers can receive from the
    /// same [tokio::net::UdpSocket].
    /// But all [ProtocolMessage]s will only arrive at one receiver at the time.
    pub async fn async_channel<C, P>(
        port: u16,
        cache: C,
        port_mapper: P,
        format: ProtocolMessageFormat,
    ) -> tokio::io::Result<(
        sender::udp_tokio::UdpSender<C>,
        receiver::udp_tokio::UdpReceiver<C, P>,
    )>
    where
        C: AsyncIpCache + Clone + Send + Sync,
        P: AsyncPortMapper + Clone + Send + Sync,
    {
        let socket = Arc::new(
            tokio::net::UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], port))).await?,
        );

        let sender = sender::udp_tokio::UdpSender::from_socket(
            socket.clone(),
            cache.clone(),
            format.clone(),
        )
        .await?;

        let receiver =
            receiver::udp_tokio::UdpReceiver::from_socket(socket, format, cache, port_mapper);

        Ok((sender, receiver))
    }
}
