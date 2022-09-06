use std::io;
use std::net::{Ipv6Addr, SocketAddr, UdpSocket};
use std::sync::Arc;

use crate::messaging::error::SenderError;
use crate::messaging::format::ProtocolMessageFormat;
use crate::messaging::{IpCache, ProtocolMessage, ProtocolMessageSender};

/// A sync [ProtocolMessageSender] implementation using UDP.
///
/// Defaults to sending the request to multicast if neighbor is not present (which should not
/// happen for physical neighbors).
///
/// Delegates the sending of messages to lower layers based on the stored IP addresses in the [IpCache].
#[derive(Debug, Clone)]
pub struct UdpSender<C> {
    format: ProtocolMessageFormat,
    socket: Arc<UdpSocket>,
    ip_cache: C,
    port: u16,
}

impl<C> UdpSender<C> {
    /// Creates a new sender and initializes a new [UdpSocket] bound to all IPv6 interfaces.
    pub fn new(
        socket_port: u16,
        broadcast_port: u16,
        ip_cache: C,
        format: ProtocolMessageFormat,
    ) -> io::Result<Self> {
        let socket = UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], socket_port)))?;

        Ok(Self {
            socket: Arc::new(socket),
            format,
            port: broadcast_port,
            ip_cache,
        })
    }

    /// Creates a new [UdpReceiver] from a socket.
    pub(crate) fn from_socket(
        socket: Arc<UdpSocket>,
        ip_cache: C,
        format: ProtocolMessageFormat,
    ) -> io::Result<Self> {
        let addr = socket.local_addr()?;
        let port = addr.port();

        Ok(Self {
            socket,
            format,
            ip_cache,
            port,
        })
    }

    fn broadcast_addr(&self) -> SocketAddr {
        SocketAddr::from((Ipv6Addr::new(0xff02, 0, 0, 0, 0, 0, 0, 1), self.port))
    }

    /// Returns the actually used local address.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }
}

impl<C: IpCache> UdpSender<C> {
    fn get_receiver_addr(&self, message: &ProtocolMessage) -> SocketAddr {
        // Due to the invariant of SourceRoutes before sending the previous hop has to be the neighbor
        let neighbor = message.current_hop();
        if let Some(neighbor) = neighbor {
            let ip = self.ip_cache.get(neighbor);

            if let Some(addr) = ip {
                return SocketAddr::from(addr);
            }
        }
        self.broadcast_addr()
    }
}

impl<C: IpCache> ProtocolMessageSender for UdpSender<C> {
    fn send<M>(&mut self, message: M) -> Result<(), SenderError>
    where
        M: Into<ProtocolMessage>,
    {
        let message = message.into();

        let mut buffer = Vec::new();
        self.format.serialize(&mut buffer, &message)?;

        let receiver_addr = self.get_receiver_addr(&message);

        log::trace!(
            target: "message_sender",
            "Sending ProtocolMessage {:?} to {}",
            &message,
            receiver_addr
        );

        self.socket
            .send_to(&buffer[..buffer.len()], receiver_addr)?;

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
    use crate::messaging::sender::udp::UdpSender;
    use crate::messaging::{HelloMessage, ProtocolMessage, ProtocolMessageSender};

    #[test]
    fn send() -> Result<(), Box<dyn Error>> {
        crate::tests::init();

        let receiver_socket = UdpSocket::bind("[::]:0").expect("failed to open socket");
        receiver_socket
            .set_read_timeout(Some(Duration::from_millis(1000)))
            .expect("failed to set read timeout");
        let addr = match receiver_socket.local_addr()? {
            SocketAddr::V6(addr) => addr,
            addr => panic!("Non-IPv6 Address: {}", addr),
        };
        log::trace!("Created receiver socket at {}", addr);

        let mut ip_cache = HashMap::new();
        ip_cache.insert(NodeId::one(), addr);
        let ip_cache = Arc::new(RwLock::new(ip_cache));

        let mut sender = UdpSender::new(0, addr.port(), ip_cache, ProtocolMessageFormat::Json)
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

        let send_result = sender.send(protocol_message);

        assert!(send_result.is_ok(), "Sending failed: {:?}", send_result);

        handle.join().expect("failed to join receiver");

        Ok(())
    }
}
