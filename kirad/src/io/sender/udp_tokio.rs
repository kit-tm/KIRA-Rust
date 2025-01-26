//! Implementations of the [AsyncProtocolMessageSender] trait using [tokio] async sockets.
//!
//! The main struct for receiving [ProtocolMessages](ProtocolMessage) is the [UdpSender].

use std::net::{SocketAddr, SocketAddrV6};
use std::sync::Arc;

use tokio::io;
use tokio::net::UdpSocket;

use super::*;
use crate::format::ProtocolMessageFormat;
use crate::io::ALL_KIRA_NODES;
use crate::underlay::UnderlayObserverHandle;

/// Defaults to sending the request to multicast if neighbor is not present (which should not
/// happen for physical neighbors).
///
/// Delegates the sending of messages to lower layers based on
/// [UnderlayNeighborInformation](crate::domain::underlay::UnderlayNeighborInformation)
#[derive(Debug, Clone)]
pub struct UdpSender {
    socket: Arc<UdpSocket>,
    format: ProtocolMessageFormat,
    underlay_handle: UnderlayObserverHandle,
    port: u16,
}

impl UdpSender {
    /// Create a new [UdpSender].
    ///
    /// This will automatically create and manage an [UdpSocket].
    pub async fn new(
        socket_port: u16,
        //broadcast_port: u16,
        underlay_handle: UnderlayObserverHandle,
        format: ProtocolMessageFormat,
    ) -> io::Result<Self> {
        let udp_socket =
            UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], socket_port))).await?;
        if let Err(err) = udp_socket.join_multicast_v6(&ALL_KIRA_NODES, 0) {
            log::trace!(target: "message_sender", "Error joining multicast group: {:?}", err);
        }

        let socket = Arc::new(udp_socket);

        Self::from_socket(socket, format, underlay_handle)
    }

    pub(crate) fn from_socket(
        socket: Arc<UdpSocket>,
        format: ProtocolMessageFormat,
        underlay_handle: UnderlayObserverHandle,
    ) -> io::Result<Self> {
        let addr = socket.local_addr()?;
        let port = addr.port();

        Ok(Self {
            socket,
            format,
            port,
            underlay_handle,
        })
    }

    /// Returns the [SocketAddr] the sender is using.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    async fn broadcast_message(&mut self, buffer: &[u8]) -> Result<(), SenderError> {
        let indices = self
            .underlay_handle
            .get_available()
            .await
            .map_err(|_| SenderError::Closed)?;
        log::trace!(target: "message_sender", "Broadcasting to {:?}", indices);
        for interface_id in indices {
            let dest = SocketAddr::V6(SocketAddrV6::new(
                ALL_KIRA_NODES,
                self.port,
                0,
                interface_id.into(),
            ));

            // FIXME investigate if we can mitigate sending to unready interfaces
            // currently only experienced in Containernet on startup
            // probably caused by interface going down in between refreshing and sending
            if let Err(e) = self
                .socket
                .send_to(buffer, dest)
                .await
                .map_err(SenderError::SendError)
            {
                log::error!(target: "message_sender", "Multicast to interface {} failed unexpectedly: {} (addr: {})", interface_id, e, ALL_KIRA_NODES);
            }
        }
        Ok(())
    }

    async fn get_receiver_addr(
        &mut self,
        destination: UnderlayNeighborDestination,
    ) -> Option<SocketAddr> {
        let addr = match destination {
            UnderlayNeighborDestination::Broadcast => return None,
            UnderlayNeighborDestination::Multicast(interface_id) => {
                SocketAddrV6::new(ALL_KIRA_NODES, self.port, 0, interface_id.into())
            }
            UnderlayNeighborDestination::UnderlayNeighbor(ulnid) => {
                let Some(neighbor) = self
                    .underlay_handle
                    .get_information(&ulnid)
                    .await
                    .expect("Sender should not be closed")
                else {
                    log::warn!(target: "message_sender",
                        "Can't determine SocketAddr for unknown underlay neighbor: {ulnid:?}"
                    );
                    return None;
                };

                SocketAddrV6::new(neighbor.ll_ipv6, self.port, 0, neighbor.interface_id.into())
            }
        };
        Some(addr.into())
    }
}

impl AsyncProtocolMessageSender for UdpSender {
    async fn send_message<M>(
        &mut self,
        message: M,
        destination: UnderlayNeighborDestination,
    ) -> Result<(), SenderError>
    where
        M: Into<ProtocolMessage> + Send + Sync,
    {
        let message = message.into();

        let mut buffer = Vec::new();
        self.format.serialize(&mut buffer, &message)?;

        if let Some(receiver_addr) = self.get_receiver_addr(destination).await {
            log::trace!(
                target: "message_sender",
                "Sending ProtocolMessage {:?} to {}",
                &message,
                receiver_addr
            );

            self.socket
                .send_to(&buffer[..buffer.len()], receiver_addr)
                .await?;
        } else {
            log::trace!(
                target: "message_sender",
                "Broadcasting ProtocolMessage {:?}",
                &message,
            );
            self.broadcast_message(&buffer).await?;
        }

        Ok(())
    }
}
