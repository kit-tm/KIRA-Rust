//! Implementations of the [AsyncProtocolMessageReceiver] trait using [tokio] async sockets.
//!
//! The main struct for receiving [ProtocolMessages](ProtocolMessage) is the [UdpReceiver].

use std::collections::HashSet;
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::ops::DerefMut;
use std::sync::Arc;

use kira_lib::domain::NodeId;
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
#[derive(derive_more::Debug)]
pub struct UdpReceiver {
    #[debug(skip)]
    buffer: RwLock<[u8; MTU_BYTES]>,
    socket: Arc<UdpSocket>,
    format: ProtocolMessageFormat,
    underlay_handle: UnderlayObserverHandle,
    excluded_interfaces: HashSet<InterfaceId>,
    root_id: NodeId,
}

impl Clone for UdpReceiver {
    /// Clones the [UdpReceiver] with a new buffer.
    fn clone(&self) -> Self {
        Self {
            buffer: RwLock::new([0u8; MTU_BYTES]),
            socket: Arc::clone(&self.socket),
            format: self.format,
            underlay_handle: self.underlay_handle.clone(),
            excluded_interfaces: self.excluded_interfaces.clone(),
            root_id: self.root_id,
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
        root_id: NodeId,
    ) -> tokio::io::Result<Self> {
        let udp_socket =
            UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], socket_port))).await?;
        if let Err(err) = udp_socket.join_multicast_v6(&ALL_KIRA_NODES, 0) {
            log::trace!(target: "message_receiver", "Error joining multicast group: {:?}", err);
        }

        let socket = Arc::new(udp_socket);

        Ok(Self::from_socket(
            socket,
            format,
            underlay_handle,
            excluded_interfaces,
            root_id,
        ))
    }

    /// Creates a new [UdpReceiver] from a given socket.
    pub(crate) fn from_socket(
        socket: Arc<UdpSocket>,
        format: ProtocolMessageFormat,
        underlay_handle: UnderlayObserverHandle,
        excluded_interfaces: HashSet<InterfaceId>,
        root_id: NodeId,
    ) -> Self {
        Self {
            buffer: RwLock::new([0u8; MTU_BYTES]),
            socket,
            format,
            underlay_handle,
            excluded_interfaces,
            root_id,
        }
    }

    fn deserialize(&self, buffer: &[u8]) -> Option<ProtocolMessage> {
        let deserialized = match self.format.deserialize(buffer) {
            Ok(message) => message,
            Err(e) => {
                log::trace!(target: "message_receiver", "Received invalid serialized message: {}", e);
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
    #[tracing::instrument(level = "debug", target = "message_receiver")]
    async fn recv(&mut self) -> Option<Result<(ProtocolMessage, UnderlayNeighborId), RecvError>> {
        let socket = Arc::clone(&self.socket);
        let mut buffer = self.buffer.write().await;

        loop {
            let (received_bytes, received_from) = match socket.recv_from(buffer.deref_mut()).await {
                Ok(received) => received,
                Err(e) => {
                    log::error!(target: "message_receiver", "Failed to receive data from socket: {}", e);
                    return Some(Err(RecvError::IoError(Box::new(e))));
                }
            };
            let received_from = match received_from {
                SocketAddr::V6(addr) => addr,
                addr => panic!("Received Non-IPv6 Packet from {}", addr),
            };

            // FIXME: ignore scope_id 0 (probably caused by ipv6 attached to lo)
            let Some(interface_id) = NonZeroU32::new(received_from.scope_id()) else {
                log::warn!(target: "message_receiver", "Ignoring message with scope_id 0");
                continue;
            };
            let interface_id = interface_id.into();

            // ignore incoming messages from excluded interfaces
            // otherwise it will error determining the ulnid
            if self.excluded_interfaces.contains(&interface_id) {
                continue;
            }

            let Some(message) = self.deserialize(&buffer[..received_bytes]) else {
                log::warn!(target: "message_receiver", "Deserialization of received message failed");
                continue;
            };
            if message.source() == &self.root_id {
                log::warn!(target: "message_receiver", "Ignoring message from us");
                continue;
            }
            log::trace!(target: "message_receiver", "Received {:?} from {}", &message, received_from);

            let ulnid = match self
                .underlay_handle
                .register_neighbor(interface_id, *received_from.ip())
                .await
            {
                Ok(ulnid) => ulnid,
                Err(UnderlayObserverHandleError::SenderClosed(e)) => {
                    log::error!(target: "message_receiver", "underlay handle sender closed: {}", e);
                    // fatal error if we can't query underlay observer anymore
                    return Some(Err(RecvError::Closed));
                }
                Err(UnderlayObserverHandleError::InterfaceDown(
                    UnderlayNeighborInterfaceDownError(id),
                )) => {
                    log::warn!(target: "message_receiver", "interface ({}) down before able to determine underlay neighbor id of received message: {:?}", id, message);

                    let mut ids = HashSet::with_capacity(1);
                    ids.insert(id);
                    return Some(Err(RecvError::InterfacesDown(ids)));
                }
            };

            return Some(Ok((message, ulnid)));
        }
    }
}
