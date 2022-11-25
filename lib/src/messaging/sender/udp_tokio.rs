use std::net::{Ipv6Addr, SocketAddr};
use std::sync::Arc;

use tokio::io;
use tokio::net::UdpSocket;

use crate::messaging::error::SenderError;
use crate::messaging::format::ProtocolMessageFormat;
use crate::messaging::{AsyncIpCache, AsyncProtocolMessageSender, ProtocolMessage};

/// Defaults to sending the request to multicast if neighbor is not present (which should not
/// happen for physical neighbors).
///
/// Delegates the sending of messages to lower layers based on the stored IP addresses in the [AsyncIpCache].
#[derive(Debug, Clone)]
pub struct UdpSender<C> {
    format: ProtocolMessageFormat,
    socket: Arc<UdpSocket>,
    ip_cache: C,
    port: u16,
}

impl<C> UdpSender<C> {
    pub async fn new(
        socket_port: u16,
        broadcast_port: u16,
        ip_cache: C,
        format: ProtocolMessageFormat,
    ) -> io::Result<Self> {
        let socket =
            UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], socket_port))).await?;

        Ok(Self {
            socket: Arc::new(socket),
            format,
            port: broadcast_port,
            ip_cache,
        })
    }

    pub(crate) async fn from_socket(
        socket: Arc<UdpSocket>,
        ip_cache: C,
        format: ProtocolMessageFormat,
    ) -> io::Result<Self> {
        let addr = socket.local_addr()?;
        let port = addr.port();

        Ok(Self {
            socket,
            format,
            port,
            ip_cache,
        })
    }

    fn broadcast_addr(&self) -> SocketAddr {
        SocketAddr::from((Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1), self.port))
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }
}

impl<C: AsyncIpCache> UdpSender<C> {
    async fn get_receiver_addr(&self, message: &ProtocolMessage) -> SocketAddr {
        // Due to the invariant of SourceRoutes before sending the previous hop has to be the neighbor
        let neighbor = message.current_hop();
        if let Some(neighbor) = neighbor {
            let ip = self.ip_cache.get(neighbor).await;

            if let Some(addr) = ip {
                return SocketAddr::from(addr);
            }
        }
        self.broadcast_addr()
    }
}

#[async_trait::async_trait]
impl<C: AsyncIpCache + Send + Sync> AsyncProtocolMessageSender for UdpSender<C> {
    async fn send_message<M>(&mut self, message: M) -> Result<(), SenderError>
    where
        M: Into<ProtocolMessage> + Send + Sync,
    {
        let message = message.into();

        let mut buffer = Vec::new();
        self.format.serialize(&mut buffer, &message)?;

        let receiver_addr = self.get_receiver_addr(&message).await;

        log::trace!(
            target: "message_sender",
            "Sending ProtocolMessage {:?} to {}",
            &message,
            receiver_addr
        );

        self.socket
            .send_to(&buffer[..buffer.len()], receiver_addr)
            .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::error::Error;
    use std::net::{SocketAddr, UdpSocket};
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::sync::RwLock;

    use crate::domain::{NodeId, StateSeqNr};
    use crate::messaging::format::ProtocolMessageFormat;
    use crate::messaging::sender::udp_tokio::UdpSender;
    use crate::messaging::{AsyncProtocolMessageSender, HelloMessage, ProtocolMessage};

    #[tokio::test]
    async fn send() -> Result<(), Box<dyn Error>> {
        crate::tests::init();

        let receiver_socket = UdpSocket::bind("[::]:0").expect("failed to open socket");
        receiver_socket
            .set_read_timeout(Some(Duration::from_millis(1000)))
            .expect("failed to set read timeout");
        let addr = match receiver_socket.local_addr()? {
            SocketAddr::V6(addr) => addr,
            addr => panic!("Non-IPv6 Address: {}", addr),
        };

        let mut ip_cache = HashMap::new();
        ip_cache.insert(NodeId::one(), addr);
        let ip_cache = Arc::new(RwLock::new(ip_cache));

        let mut sender = UdpSender::new(0, addr.port(), ip_cache, ProtocolMessageFormat::Json)
            .await
            .expect("failed to create sender");

        let handle = std::thread::spawn(move || {
            let mut buffer = [0u8; 65536];

            loop {
                let received = receiver_socket.recv_from(&mut buffer);
                assert!(received.is_ok(), "{:?}", received);
                let (received_bytes, _) = received.unwrap();

                let message = ProtocolMessageFormat::Json.deserialize(&buffer[..received_bytes]);
                assert!(message.is_ok(), "{:?}", message);

                let message = message.unwrap();
                assert_eq!(
                    message,
                    ProtocolMessage::Hello(HelloMessage {
                        source: NodeId::one(),
                        source_state_seq_nr: StateSeqNr::from(0),
                    })
                );
                break;
            }
        });

        let protocol_message = ProtocolMessage::Hello(HelloMessage {
            source: NodeId::one(),
            source_state_seq_nr: StateSeqNr::from(0),
        });

        let send_result = sender.send_message(protocol_message).await;

        assert!(send_result.is_ok(), "{:?}", send_result);

        handle.join().expect("failed to join receiver");

        Ok(())
    }
}
