//! Implementations of the [AsyncProtocolMessageReceiver] trait using [tokio] async sockets.
//!
//! The main struct for receiving [ProtocolMessages](ProtocolMessage) is the [UdpReceiver].

use std::collections::HashSet;
use std::fmt::Debug;
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::ops::DerefMut;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::sync::RwLock;

use super::*;
use crate::format::ProtocolMessageFormat;
use crate::io::ALL_KIRA_NODES;
use crate::underlay::handle::UnderlayObserverHandleError;
use crate::underlay::UnderlayNeighborInterfaceDownError;
use crate::underlay::UnderlayObserverHandle;

/// Maximum Transmission Unit (MTU). In general the MTU is actually smaller due to
/// network restrictions. But to be safe we use this.
const MTU_BYTES: usize = 65536;

/// An [AsyncProtocolMessageReceiver] implementation using UDP.
///
/// The buffer used for messages is not shared between (cloned) instances of [UdpReceiver].
///
/// The used [UdpSocket] binds to all available IPv6 interfaces and maps the incoming IP
/// addresses to [UnderlayNeighborIds][UnderlayNeighborId] using an [UnderlayObserverHandle].
#[derive(Debug)]
pub struct UdpReceiver {
    buffer: RwLock<[u8; MTU_BYTES]>,
    socket: Arc<UdpSocket>,
    format: ProtocolMessageFormat,
    underlay_handle: UnderlayObserverHandle,
    excluded_interfaces: HashSet<InterfaceId>,
}

impl Clone for UdpReceiver {
    /// Clones the [UdpReceiver] with a new buffer.
    fn clone(&self) -> Self {
        Self {
            buffer: RwLock::new([0u8; MTU_BYTES]),
            socket: Arc::clone(&self.socket),
            format: self.format.clone(),
            underlay_handle: self.underlay_handle.clone(),
            excluded_interfaces: self.excluded_interfaces.clone(),
        }
    }
}

impl UdpReceiver {
    /// Creates a new [UdpReceiver].
    ///
    /// Initializes the internally used [UdpSocket].
    ///
    /// To bind the receiver to a random free interface, use `socket_port = 0`.
    pub async fn new(
        socket_port: u16,
        format: ProtocolMessageFormat,
        underlay_handle: UnderlayObserverHandle,
        excluded_interfaces: HashSet<InterfaceId>,
    ) -> tokio::io::Result<Self> {
        let udp_socket =
            UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], socket_port))).await?;
        if let Err(err) = udp_socket.join_multicast_v6(&ALL_KIRA_NODES, 0) {
            log::trace!("Error joining multicast group: {:?}", err);
        }

        let socket = Arc::new(udp_socket);

        Ok(Self::from_socket(
            socket,
            format,
            underlay_handle,
            excluded_interfaces,
        ))
    }

    /// Creates a new [UdpReceiver] from a given socket.
    pub(crate) fn from_socket(
        socket: Arc<UdpSocket>,
        format: ProtocolMessageFormat,
        underlay_handle: UnderlayObserverHandle,
        excluded_interfaces: HashSet<InterfaceId>,
    ) -> Self {
        Self {
            buffer: RwLock::new([0u8; MTU_BYTES]),
            socket,
            format,
            underlay_handle,
            excluded_interfaces,
        }
    }

    fn deserialize(&self, buffer: &[u8]) -> Option<ProtocolMessage> {
        let deserialized = match self.format.deserialize(buffer) {
            Ok(message) => message,
            Err(e) => {
                log::trace!("Received invalid serialized message: {}", e);
                return None;
            }
        };

        Some(deserialized)
    }

    /// Returns the actually bound local address.
    pub fn local_addr(&self) -> tokio::io::Result<SocketAddr> {
        self.socket.local_addr()
    }
}

impl AsyncProtocolMessageReceiver for UdpReceiver {
    async fn recv_timeout(
        &mut self,
        timeout: Option<Duration>,
    ) -> Result<Option<(ProtocolMessage, UnderlayNeighborId)>, RecvError> {
        let socket = Arc::clone(&self.socket);
        let mut buffer = self.buffer.write().await;

        let receive_with_optional_timeout = if let Some(duration) = timeout {
            tokio::time::timeout(duration, socket.recv_from(buffer.deref_mut()))
                .await
                .map_err(|_| RecvError::Timeout)?
        } else {
            socket.recv_from(buffer.deref_mut()).await
        };

        let (received_bytes, received_from) = match receive_with_optional_timeout {
            Ok(received) => received,
            Err(e) => {
                log::error!("Failed to receive data from socket: {}", e);
                return Err(RecvError::IoError(Box::new(e)));
            }
        };
        let received_from = match received_from {
            SocketAddr::V6(addr) => addr,
            addr => panic!("Received Non-IPv6 Packet from {}", addr),
        };

        // FIXME: ignore scope_id 0 (probably caused by ipv6 attached to lo)
        let Some(interface_id) = NonZeroU32::new(received_from.scope_id()) else {
            log::warn!("Ignoring message with scope_id 0");
            return Ok(None);
        };
        let interface_id = interface_id.into();

        // ignore incoming messages from excluded interfaces
        if self.excluded_interfaces.contains(&interface_id) {
            return Ok(None);
        }

        let Some(message) = self.deserialize(&buffer[..received_bytes]) else {
            log::warn!("Deserialization of received message failed");
            return Ok(None);
        };
        log::trace!(target: "message_receiver", "Received {:?} from {}", &message, received_from);

        let ulnid = match self
            .underlay_handle
            .register_neighbor(interface_id, *received_from.ip())
            .await
        {
            Ok(ulnid) => ulnid,
            Err(UnderlayObserverHandleError::SenderClosed(e)) => {
                log::error!("underlay handle sender closed: {}", e);
                return Err(RecvError::Other(Box::new(e)));
            }
            Err(UnderlayObserverHandleError::InterfaceDown(
                UnderlayNeighborInterfaceDownError(id),
            )) => {
                log::warn!("interface ({}) down before able to determine underlay neighbor id of received message: {:?}", id, message);

                let mut ids = HashSet::with_capacity(1);
                ids.insert(id);
                return Err(RecvError::InterfacesDown(ids));
            }
        };

        Ok(Some((message, ulnid)))
    }

    async fn recv(&mut self) -> Result<Option<(ProtocolMessage, UnderlayNeighborId)>, RecvError> {
        AsyncProtocolMessageReceiver::recv_timeout(self, None).await
    }

    async fn try_recv(
        &mut self,
    ) -> Result<Option<(ProtocolMessage, UnderlayNeighborId)>, TryRecvError> {
        // TODO: Remove duplicate code

        let mut buffer = self.buffer.write().await;

        let (received_bytes, received_from) = match self.socket.try_recv_from(buffer.deref_mut()) {
            Ok(received) => received,
            Err(e) => return Err(TryRecvError::IoError(Box::new(e))),
        };

        let received_from = match received_from {
            SocketAddr::V6(addr) => addr,
            addr => panic!("Received Non-IPv6 Packet from {}", addr),
        };

        // FIXME: ignore scope_id 0 (probably caused by ipv6 attached to lo)
        let Some(interface_id) = NonZeroU32::new(received_from.scope_id()) else {
            log::warn!("Ignoring message with scope_id 0");
            return Ok(None);
        };
        let interface_id = interface_id.into();

        // ignore incoming messages from excluded interfaces
        if self.excluded_interfaces.contains(&interface_id) {
            return Ok(None);
        }

        let Some(message) = self.deserialize(&buffer[..received_bytes]) else {
            log::warn!("Deserialization of received message failed");
            return Ok(None);
        };
        log::trace!(target: "message_receiver", "Received {:?} from {}", &message, received_from);

        let ulnid = self
            .underlay_handle
            .register_neighbor(interface_id, *received_from.ip())
            .await
            .unwrap();

        Ok(Some((message, ulnid)))
    }
}
