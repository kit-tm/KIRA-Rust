//! Implementations of the [AsyncProtocolMessageSender] trait using [tokio] async sockets.
//!
//! The main struct for receiving [ProtocolMessages](ProtocolMessage) is the [UdpSender].

use std::io;
use std::net::{SocketAddr, SocketAddrV6};
use std::sync::Arc;

use tokio::net::UdpSocket;
use tracing::Level;

use super::*;
use crate::format::ProtocolMessageFormat;
use crate::io::ALL_KIRA_NODES;
use crate::underlay::UnderlayObserverHandle;

/// Defaults to sending the request to multicast if neighbor is not present (which should not
/// happen for physical neighbors).
///
/// Delegates the sending of messages to lower layers based on
/// [UnderlayNeighborInformation](kira_forwarding::underlay::UnderlayNeighborInformation)
#[derive(Debug, Clone)]
pub struct UdpSender {
    socket: Arc<UdpSocket>,
    format: ProtocolMessageFormat,
    underlay_handle: UnderlayObserverHandle,
    port: u16,
}

impl UdpSender {
    pub(crate) fn from_socket(
        socket: Arc<UdpSocket>,
        port: u16,
        format: ProtocolMessageFormat,
        underlay_handle: UnderlayObserverHandle,
    ) -> Self {
        Self {
            socket,
            format,
            port,
            underlay_handle,
        }
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
        tracing::trace!(target: "message_sender", "Broadcasting to {indices:?}");
        for interface_id in indices {
            let dest = SocketAddr::V6(SocketAddrV6::new(
                ALL_KIRA_NODES,
                self.port,
                0,
                interface_id.into(),
            ));

            // FIXME: investigate if we can mitigate sending to unready interfaces
            // currently only experienced in Containernet on startup
            // probably caused by interface going down in between refreshing and sending
            if let Err(err) = self
                .socket
                .send_to(buffer, dest)
                .await
                .map_err(SenderError::SendError)
            {
                tracing::error!(
                    target: "message_sender",
                    interface = %interface_id,
                    %err,
                    "Multicast to interface failed unexpectedly (addr: {ALL_KIRA_NODES})");
            }
        }
        Ok(())
    }

    #[tracing::instrument(
        level = Level::TRACE,
        target = "message_sender",
        skip(self),
        ret(level = Level::TRACE),
    )]
    async fn get_receiver_addr(
        &mut self,
        destination: UnderlayNeighborDestination,
    ) -> Result<Option<SocketAddr>, SenderError> {
        Ok(match destination {
            UnderlayNeighborDestination::Broadcast => None,
            UnderlayNeighborDestination::Multicast(interface_id) => {
                Some(SocketAddrV6::new(ALL_KIRA_NODES, self.port, 0, interface_id.into()).into())
            }
            UnderlayNeighborDestination::UnderlayNeighbor(ulnid) => {
                let Some(neighbor) = self
                    .underlay_handle
                    .get_information(&ulnid)
                    .await
                    .map_err(|err| {
                        tracing::error!(target: "message_sender", %err, "Underlay handle sender closed");
                        SenderError::Closed
                    })?
                else {
                    tracing::warn!(
                        target: "message_sender",
                        %ulnid,
                        fallback = "Broadcasting",
                        "Can't determine SocketAddr for unknown underlay neighbor",
                    );
                    return Ok(None);
                };

                Some(
                    SocketAddrV6::new(neighbor.ll_ipv6, self.port, 0, neighbor.interface_id.into())
                        .into(),
                )
            }
        })
    }
}

impl AsyncProtocolMessageSender for UdpSender {
    #[tracing::instrument(
        level = Level::TRACE,
        target = "message_sender",
        skip(self),
        fields(
            port = ?self.port,
            format = ?self.format)
    )]
    async fn send_message<M>(
        &mut self,
        message: M,
        destination: UnderlayNeighborDestination,
    ) -> Result<(), SenderError>
    where
        M: Into<ProtocolMessage> + Send + Sync + std::fmt::Debug,
    {
        let message = message.into();

        let mut buffer = Vec::new();
        self.format.serialize(&mut buffer, &message)?;

        if let Some(receiver_addr) = self.get_receiver_addr(destination).await? {
            tracing::trace!( target: "message_sender", %receiver_addr, "Sending ProtocolMessage");

            self.socket
                .send_to(&buffer[..buffer.len()], receiver_addr)
                .await?;
        } else {
            tracing::trace!( target: "message_sender", "Broadcasting ProtocolMessage");
            self.broadcast_message(&buffer).await?;
        }

        Ok(())
    }
}
