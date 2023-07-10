use std::fmt::Debug;
use std::net::{Ipv6Addr, SocketAddr};
use std::ops::DerefMut;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::sync::RwLock;

use crate::domain::NetworkInterface;
use crate::messaging::format::ProtocolMessageFormat;
use crate::messaging::{
    AsyncInterfaceMapper, AsyncIpCache, AsyncProtocolMessageReceiver, ProtocolMessage, RecvError,
    TryRecvError,
};

/// Maximum Transmission Unit (MTU). In general the MTU is actually smaller due to
/// network restrictions. But to be safe we use this.
const MTU_BYTES: usize = 65536;

/// An [AsyncProtocolMessageReceiver] implementation using UDP.
///
/// The buffer used for messages is not shared between (cloned) instances of [UdpReceiver].
///
/// The used [UdpSocket] binds to all available IPv6 interfaces and maps the incoming IP
/// addresses to the ports using the generic parameter P ([InterfaceMapper](crate::messaging::InterfaceMapper)).
#[derive(Debug)]
pub struct UdpReceiver<C, P> {
    buffer: RwLock<[u8; MTU_BYTES]>,
    socket: Arc<UdpSocket>,
    format: ProtocolMessageFormat,
    interface_mapper: P,
    ip_cache: C,
}

impl<C: Clone, P: Clone> Clone for UdpReceiver<C, P> {
    /// Clones the [UdpReceiver] with a new buffer.
    fn clone(&self) -> Self {
        Self {
            buffer: RwLock::new([0u8; MTU_BYTES]),
            socket: Arc::clone(&self.socket),
            format: self.format.clone(),
            interface_mapper: self.interface_mapper.clone(),
            ip_cache: self.ip_cache.clone(),
        }
    }
}

impl<C, P> UdpReceiver<C, P> {
    /// Creates a new [UdpReceiver].
    ///
    /// Initializes the internally used [UdpSocket].
    ///
    /// To bind the receiver to a random free interface, use `socket_port = 0`.
    pub async fn new(
        socket_port: u16,
        format: ProtocolMessageFormat,
        ip_cache: C,
        interface_mapper: P,
    ) -> tokio::io::Result<Self> {
        let udp_socket =
            UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], socket_port))).await?;
        if let Err(err) =
            udp_socket.join_multicast_v6(&Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1), 0)
        {
            log::trace!("Error joining multicast group: {:?}", err);
        }

        let socket = Arc::new(udp_socket);

        Ok(Self {
            buffer: RwLock::new([0u8; MTU_BYTES]),
            socket,
            format,
            interface_mapper,
            ip_cache,
        })
    }

    /// Creates a new [UdpReceiver] from a given socket.
    pub(crate) fn from_socket(
        socket: Arc<UdpSocket>,
        format: ProtocolMessageFormat,
        ip_cache: C,
        port_mapper: P,
    ) -> Self {
        Self {
            buffer: RwLock::new([0u8; MTU_BYTES]),
            socket,
            format,
            interface_mapper: port_mapper,
            ip_cache,
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

#[async_trait::async_trait]
impl<C, P> AsyncProtocolMessageReceiver for UdpReceiver<C, P>
where
    P: AsyncInterfaceMapper + Send + Sync,
    C: AsyncIpCache + Send + Sync,
{
    async fn recv_timeout(
        &mut self,
        timeout: Option<Duration>,
    ) -> Result<Option<(ProtocolMessage, NetworkInterface)>, RecvError> {
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

        let message = self.deserialize(&buffer[..received_bytes]);
        let interface = self
            .interface_mapper
            .get_interface(&received_from)
            .await
            .ok_or(RecvError::NoInterfaceFound)?;

        if let Some(message) = &message {
            log::trace!(target: "message_receiver", "Received {:?} from {}", &message, received_from);

            let previous_node = message.previous_hop();

            let received_from = match received_from {
                SocketAddr::V6(addr) => addr,
                addr => panic!("Received Non-IPv6 Packet from {}", addr),
            };

            if let Some(ip) = self
                .ip_cache
                .insert(previous_node.clone(), received_from)
                .await
            {
                if ip != received_from {
                    log::trace!(
                        "Replaced ip for {} ({} => {})",
                        previous_node,
                        ip,
                        received_from.ip()
                    );
                }
            }
        }

        Ok(message.map(|message| (message, interface)))
    }

    async fn recv(&mut self) -> Result<Option<(ProtocolMessage, NetworkInterface)>, RecvError> {
        self.recv_timeout(None).await
    }

    async fn try_recv(
        &mut self,
    ) -> Result<Option<(ProtocolMessage, NetworkInterface)>, TryRecvError> {
        let mut buffer = self.buffer.write().await;
        let (received_bytes, address) = match self.socket.try_recv_from(buffer.deref_mut()) {
            Ok(received) => received,
            Err(e) => return Err(TryRecvError::IoError(Box::new(e))),
        };

        let message = self.deserialize(&buffer[..received_bytes]);
        let interface = self
            .interface_mapper
            .get_interface(&address)
            .await
            .ok_or(TryRecvError::NoInterfaceFound)?;

        Ok(message.map(|message| (message, interface)))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::error::Error;
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::net::UdpSocket;
    use tokio::sync::RwLock;

    use crate::domain::{NodeId, StateSeqNr};
    use crate::messaging::format::ProtocolMessageFormat;
    use crate::messaging::receiver::udp_tokio::UdpReceiver;
    use crate::messaging::{
        AsyncProtocolMessageReceiver, HelloMessage, PNetInterfaceMonitor, ProtocolMessage,
    };

    #[tokio::test]
    async fn receive_timeout() -> Result<(), Box<dyn Error + Send + Sync>> {
        crate::tests::init();

        let cache = Arc::new(RwLock::new(HashMap::new()));

        let mut receiver = UdpReceiver::new(
            0,
            ProtocolMessageFormat::Json,
            cache,
            PNetInterfaceMonitor::new(),
        )
        .await
        .expect("failed to start udp receiver");
        let addr = receiver.local_addr()?;

        let handle = tokio::spawn(async move {
            let socket = UdpSocket::bind("[::]:0").await?;
            // may need to join multicast group

            let protocol_message = ProtocolMessage::Hello(HelloMessage {
                source: NodeId::one(),
                source_state_seq_nr: StateSeqNr::from(0),
            });

            let mut buffer = Vec::new();

            ProtocolMessageFormat::Json.serialize(&mut buffer, &protocol_message)?;

            socket.send_to(&buffer[..buffer.len()], addr).await?;

            Result::<(), Box<dyn Error + Send + Sync>>::Ok(())
        });

        let received = receiver
            .recv_timeout(Some(Duration::from_micros(500)))
            .await;

        handle.await.expect("failed to join")?;

        assert!(received.is_ok(), "{:?}", received);
        let received = received.unwrap();
        assert!(received.is_some(), "{:?}", received);
        let (message, _) = received.unwrap();
        assert_eq!(
            message,
            ProtocolMessage::Hello(HelloMessage {
                source: NodeId::one(),
                source_state_seq_nr: StateSeqNr::from(0),
            })
        );

        Ok(())
    }

    #[tokio::test]
    async fn try_receive() -> Result<(), Box<dyn Error + Send + Sync>> {
        crate::tests::init();

        let cache = Arc::new(RwLock::new(HashMap::new()));

        let mut receiver = UdpReceiver::new(
            0,
            ProtocolMessageFormat::Json,
            cache,
            PNetInterfaceMonitor::new(),
        )
        .await
        .expect("failed to create udp receiver");
        let addr = receiver.local_addr()?;

        let handle = tokio::spawn(async move {
            let socket = UdpSocket::bind("[::]:0").await?;
            // may need to join multicast group

            let protocol_message = ProtocolMessage::Hello(HelloMessage {
                source: NodeId::one(),
                source_state_seq_nr: StateSeqNr::from(0),
            });

            let mut buffer = Vec::new();

            ProtocolMessageFormat::Json.serialize(&mut buffer, &protocol_message)?;

            socket.send_to(&buffer[..buffer.len()], addr).await?;

            Result::<(), Box<dyn Error + Send + Sync>>::Ok(())
        });

        handle.await.expect("failed to join")?;

        let received = receiver.try_recv().await;

        assert!(received.is_ok(), "{:?}", received);
        let received = received.unwrap();
        assert!(received.is_some(), "{:?}", received);
        let (message, _) = received.unwrap();
        assert_eq!(
            message,
            ProtocolMessage::Hello(HelloMessage {
                source: NodeId::one(),
                source_state_seq_nr: StateSeqNr::from(0),
            })
        );

        Ok(())
    }
}
