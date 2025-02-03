//! Type definitions containing everything related to [ProtocolMessage](kira_r2kad::messaging::ProtocolMessage) transmission.

use std::net::Ipv6Addr;

pub mod receiver;
pub mod sender;

/// Well-known link-local multicast address `ALL-KIRA-NODES`.
///
/// This should reach all nodes running a KIRA instance
pub const ALL_KIRA_NODES: Ipv6Addr = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1);

/// Utilities for sending and receiving [ProtocolMessages](kira_r2kad::messaging::ProtocolMessage)
/// using async [tokio] channels.
#[cfg(feature = "udp-tokio")]
pub mod udp {
    use kira_r2kad::domain::{InterfaceId, NodeId};
    use std::collections::HashSet;
    use std::net::SocketAddr;
    use std::sync::Arc;

    use tokio::net::UdpSocket;

    use crate::format::ProtocolMessageFormat;
    use crate::io::ALL_KIRA_NODES;
    use crate::io::{receiver::udp_tokio::UdpReceiver, sender::udp_tokio::UdpSender};
    use crate::underlay::UnderlayObserverHandle;

    /// Creates a asynchronous I/O Channel consisting of one [UdpSender](super::sender::udp_tokio::UdpSender) and
    /// one [UdpReceiver](super::receiver::udp_tokio::UdpReceiver) with UDP implementations.
    ///
    /// The [UdpSender](super::sender::udp_tokio::UdpSender) and [UdpReceiver](super::receiver::udp_tokio::UdpReceiver) share the same
    /// [tokio::net::UdpSocket].
    /// This way multiple senders can send and multiple receivers can receive from the
    /// same [tokio::net::UdpSocket].
    /// But all [ProtocolMessages](kira_r2kad::messaging::ProtocolMessage) will only arrive
    /// *at one receiver* at the time.
    pub async fn async_channel(
        port: u16,
        format: ProtocolMessageFormat,
        underlay_handle: UnderlayObserverHandle,
        excluded_interfaces: HashSet<InterfaceId>,
        root_id: NodeId,
    ) -> tokio::io::Result<(UdpSender, UdpReceiver)> {
        let udp_socket =
            UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], port))).await?;
        if let Err(err) = udp_socket.join_multicast_v6(&ALL_KIRA_NODES, 0) {
            tracing::warn!(error = ?err, "Error joining multicast group");
        }
        let socket = Arc::new(udp_socket);
        let sender = UdpSender::from_socket(socket.clone(), format, underlay_handle.clone())?;
        let receiver = UdpReceiver::from_socket(
            socket,
            format,
            underlay_handle,
            excluded_interfaces,
            root_id,
        );
        Ok((sender, receiver))
    }
}
