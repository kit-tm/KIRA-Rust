//! Implementations of the [AsyncProtocolMessageReceiver] trait using [tokio] async sockets.
//!
//! The main struct for receiving [ProtocolMessages](ProtocolMessage) is the [UdpReceiver].

use std::collections::HashSet;
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::sync::Arc;

use kira_r2kad::domain::NodeId;
use kira_r2kad::messaging::ProtocolMessageKind;
use tokio::net::UdpSocket;

use super::*;
use crate::domain::underlay::UnderlayNeighbor;
use crate::format::ProtocolMessageFormat;
use crate::underlay::UnderlayObserverHandle;
use crate::underlay::handle::UnderlayObserverHandleError;
use crate::underlay::information_base::UnderlayNeighborInterfaceDownError;

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
    buffer: Box<[u8; MTU_BYTES]>,
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
            buffer: Box::new([0u8; MTU_BYTES]),
            socket: Arc::clone(&self.socket),
            format: self.format,
            underlay_handle: self.underlay_handle.clone(),
            excluded_interfaces: self.excluded_interfaces.clone(),
            root_id: self.root_id,
        }
    }
}

impl UdpReceiver {
    /// Creates a new [UdpReceiver] from a given socket.
    pub(crate) fn from_socket(
        socket: Arc<UdpSocket>,
        format: ProtocolMessageFormat,
        underlay_handle: UnderlayObserverHandle,
        excluded_interfaces: HashSet<InterfaceId>,
        root_id: NodeId,
    ) -> Self {
        // Don't receive multicast ULNHello the node sent itself.
        if let Err(err) = socket.set_multicast_loop_v6(false) {
            tracing::warn!(%err, "Failed to disable multicast IPv6 loopback");
            // no error since we still drop them if received
        }

        Self {
            buffer: Box::new([0u8; MTU_BYTES]),
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
                log::error!(target: "message_receiver", "Received invalid serialized message: {e}");
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
        loop {
            let (received_bytes, received_from) = match socket
                .recv_from(self.buffer.as_mut_slice())
                .await
            {
                Ok(received) => received,
                Err(e) => {
                    log::error!(target: "message_receiver", "Failed to receive data from socket: {e}");
                    return Some(Err(RecvError::IoError(Box::new(e))));
                }
            };
            let received_from = match received_from {
                SocketAddr::V6(addr) => addr,
                addr => panic!("Received Non-IPv6 Packet from {addr}"),
            };

            // FIXME: ignore scope_id 0 (probably caused by ipv6 attached to lo)
            let Some(interface_id) = NonZeroU32::new(received_from.scope_id()) else {
                log::trace!(target: "message_receiver", "Ignoring message with scope_id 0");
                continue;
            };
            let interface_id = interface_id.into();

            // ignore incoming messages from excluded interfaces
            // otherwise it will error determining the ulnid
            if self.excluded_interfaces.contains(&interface_id) {
                continue;
            }

            let Some(message) = self.deserialize(&self.buffer[..received_bytes]) else {
                log::error!(target: "message_receiver", "Deserialization of received message failed");
                continue;
            };

            if message.kind() == ProtocolMessageKind::ULNHello && message.source() == &self.root_id
            {
                // IPV6_MULTICAST_LOOP should be set
                std::hint::cold_path();
                tracing::warn!(
                    target: "message_receiver",
                    ?message,
                    reason = "Ignoring ULNHello from us",
                    "Dropping message",
                );
                continue;
            }
            log::trace!(target: "message_receiver", "Received {:?} from {}", message, received_from);

            let neighbor = UnderlayNeighbor::new(*received_from.ip(), interface_id);
            let ulnid = match self.underlay_handle.register_neighbor(neighbor).await {
                Ok(ulnid) => ulnid,
                Err(UnderlayObserverHandleError::SenderClosed(e)) => {
                    log::error!(target: "message_receiver", "underlay handle sender closed: {e}");
                    // fatal error if we can't query underlay observer anymore
                    return Some(Err(RecvError::Closed));
                }
                Err(UnderlayObserverHandleError::InterfaceDown(
                    UnderlayNeighborInterfaceDownError(id),
                )) => {
                    log::warn!(target: "message_receiver", "interface ({id}) down before able to determine underlay neighbor id of received message: {message:?}");

                    let mut ids = HashSet::with_capacity(1);
                    ids.insert(id);
                    return Some(Err(RecvError::InterfacesDown(ids)));
                }
            };

            return Some(Ok((message, ulnid)));
        }
    }
}
