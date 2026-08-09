//! Type definitions containing everything related to [ProtocolMessage](kira_r2kad::messaging::ProtocolMessage) transmission.

use std::{
    io,
    net::{
        Ipv6Addr,
        SocketAddr,
    },
};

use derive_more::{
    Display,
    Error,
};

pub mod receiver;
pub mod sender;

/// Well-known link-local multicast address `ALL-KIRA-NODES`.
///
/// This should reach all nodes running a KIRA instance
pub const ALL_KIRA_NODES: Ipv6Addr = Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1);

#[derive(Debug, Display, Error)]
/// Fatal errors when creating a socket listening for R²/KAD protocol messages.
pub enum SocketCreationErr {
    /// Failed to bind socket to address.
    #[display("Failed to bind socket to address {_1}")]
    BindFailed(io::Error, SocketAddr),
    /// Failed to join [`ALL_KIRA_NODES`] multicast address.
    #[display("Error joining ALL-KIRA-NODES mulicast group")]
    MulticastJoinFailed(io::Error),
}

/// Utilities for sending and receiving [ProtocolMessages](kira_r2kad::messaging::ProtocolMessage)
/// using async [tokio] channels.
#[cfg(feature = "udp-tokio")]
pub mod udp {
    use std::{
        collections::HashSet,
        net::SocketAddr,
        sync::Arc,
    };

    use kira_r2kad::domain::{
        InterfaceId,
        NodeId,
    };
    use tokio::net::UdpSocket;

    use crate::{
        format::ProtocolMessageFormat,
        io::{
            ALL_KIRA_NODES,
            SocketCreationErr,
            receiver::udp_tokio::UdpReceiver,
            sender::udp_tokio::UdpSender,
        },
        underlay::UnderlayObserverHandle,
    };

    /// Creates a asynchronous I/O Channel consisting of one [UdpSender] and
    /// one [UdpReceiver] with UDP implementations.
    ///
    /// The [UdpSender] and [UdpReceiver] share the same
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
    ) -> Result<(UdpSender, UdpReceiver), SocketCreationErr> {
        let socket_addr = SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], port));
        let udp_socket = UdpSocket::bind(socket_addr)
            .await
            .map_err(|e| SocketCreationErr::BindFailed(e, socket_addr))?;
        udp_socket
            .join_multicast_v6(&ALL_KIRA_NODES, 0)
            .map_err(SocketCreationErr::MulticastJoinFailed)?;
        let socket = Arc::new(udp_socket);
        let sender = UdpSender::from_socket(socket.clone(), port, format, underlay_handle.clone());
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
