use std::fmt::Debug;
use std::io;
use std::io::ErrorKind;
use std::net::{SocketAddr, UdpSocket};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::domain::NetworkInterface;
use crate::messaging::format::ProtocolMessageFormat;
use crate::messaging::{
    InterfaceMapper, IpCache, ProtocolMessage, ProtocolMessageReceiver, RecvError, TryRecvError,
};

/// Maximum Transmission Unit (MTU). In general the MTU is actually smaller due to
/// network restrictions. But to be safe we use this.
const MTU_BYTES: usize = 65536;

/// A sync [ProtocolMessageReceiver] implementation using UDP.
///
/// The buffer used for messages is not shared between (cloned) instances of [UdpReceiver].
///
/// The used [UdpSocket] binds to all available IPv6 interfaces and maps the incoming IP
/// addresses to the ports using the generic parameter P ([PortMapper]).
#[derive(Debug)]
pub struct UdpReceiver<C: Debug, P: Debug> {
    buffer: RwLock<[u8; MTU_BYTES]>,
    socket: Arc<UdpSocket>,
    format: ProtocolMessageFormat,
    ip_cache: C,
    interface_mapper: P,
}

impl<C: Debug + Clone, P: Debug + Clone> Clone for UdpReceiver<C, P> {
    /// Clones the [UdpReceiver] with a new buffer.
    fn clone(&self) -> Self {
        Self {
            buffer: RwLock::new([0u8; MTU_BYTES]),
            socket: Arc::clone(&self.socket),
            format: self.format.clone(),
            ip_cache: self.ip_cache.clone(),
            interface_mapper: self.interface_mapper.clone(),
        }
    }
}

impl<C: Debug, P: Debug> UdpReceiver<C, P> {
    /// Creates a new [UdpReceiver].
    ///
    /// Initializes the internally used [UdpSocket].
    ///
    /// To bind the receiver to a random free interface, use `socket_port = 0`.
    pub fn new(
        socket_port: u16,
        ip_cache: C,
        interface_mapper: P,
        format: ProtocolMessageFormat,
    ) -> io::Result<Self> {
        let socket = Arc::new(UdpSocket::bind(SocketAddr::from((
            [0, 0, 0, 0, 0, 0, 0, 0],
            socket_port,
        )))?);

        Ok(Self {
            buffer: RwLock::new([0u8; MTU_BYTES]),
            socket,
            format,
            interface_mapper,
            ip_cache,
        })
    }

    /// Creates a new [UdpReceiver] from a socket.
    pub(crate) fn from_socket(
        socket: Arc<UdpSocket>,
        ip_cache: C,
        interface_mapper: P,
        format: ProtocolMessageFormat,
    ) -> Self {
        Self {
            buffer: RwLock::new([0u8; MTU_BYTES]),
            socket,
            format,
            interface_mapper,
            ip_cache,
        }
    }

    fn deserialize(&self, buffer: &[u8]) -> Option<ProtocolMessage> {
        let deserialized = match self.format.deserialize(buffer) {
            Ok(message) => message,
            Err(e) => {
                log::debug!("Received invalid serialized message: {}", e);
                return None;
            }
        };

        Some(deserialized)
    }

    /// Returns the actually bound local address.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }
}

impl<C: Debug + IpCache, P: Debug + InterfaceMapper> ProtocolMessageReceiver for UdpReceiver<C, P> {
    fn recv_timeout(
        &mut self,
        mut timeout: Option<Duration>,
    ) -> Result<Option<(ProtocolMessage, NetworkInterface)>, RecvError> {
        let socket = Arc::clone(&self.socket);
        let mut buffer = self.buffer.write().expect("failed to get write lock");

        timeout = timeout.and_then(|dur| if dur.is_zero() { None } else { Some(dur) });
        let read_timeout_before = socket
            .read_timeout()
            .map_err(|e| RecvError::IoError(Box::new(e)))?;
        socket
            .set_read_timeout(timeout)
            .expect("invalid timeout set"); // Shouldn't happen due to previous check

        let receive_with_optional_timeout = socket.recv_from(&mut buffer[..]);

        // Reset settings after receive
        if let Err(e) = socket.set_read_timeout(read_timeout_before) {
            return Err(RecvError::IoError(Box::new(e)));
        }

        let (received_bytes, received_from) = match receive_with_optional_timeout {
            Ok(bytes) => bytes,
            Err(e) => {
                return match e.kind() {
                    ErrorKind::WouldBlock | ErrorKind::TimedOut => Err(RecvError::Timeout),
                    _ => {
                        log::error!("Failed to receive message from udp: {}", e);
                        Err(RecvError::IoError(Box::new(e)))
                    }
                };
            }
        };

        let message = self.deserialize(&buffer[..received_bytes]);

        let interface = self
            .interface_mapper
            .get_interface(&received_from)
            .ok_or(RecvError::NoInterfaceFound)?;

        if let Some(message) = &message {
            let previous_node = message.previous_hop();

            let received_from = match received_from {
                SocketAddr::V6(addr) => addr,
                addr => panic!("Received Non-IPv6 Packet from {}", addr),
            };

            if let Some(ip) = self.ip_cache.insert(previous_node.clone(), received_from) {
                log::trace!(
                    "Replaced ip for {} ({} => {})",
                    previous_node,
                    ip,
                    received_from.ip()
                );
            }
        }

        Ok(message.map(|message| (message, interface)))
    }

    fn try_recv(&mut self) -> Result<Option<(ProtocolMessage, NetworkInterface)>, TryRecvError> {
        let mut buffer = self
            .buffer
            .write()
            .expect("failed to get write lock on buffer");

        self.socket
            .set_nonblocking(true)
            .map_err(|e| TryRecvError::IoError(Box::new(e)))?;

        let (received_bytes, received_addr) = match self.socket.recv_from(&mut buffer[..]) {
            Ok(received) => received,
            Err(e) => return Err(TryRecvError::IoError(Box::new(e))),
        };

        self.socket
            .set_nonblocking(false)
            .map_err(|e| TryRecvError::IoError(Box::new(e)))?;

        let message = self.deserialize(&buffer[..received_bytes]);

        let interface = self
            .interface_mapper
            .get_interface(&received_addr)
            .ok_or(TryRecvError::NoInterfaceFound)?;

        Ok(message.map(|message| (message, interface)))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::error::Error;
    use std::net::UdpSocket;
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::sync::RwLock;

    use crate::domain::{NodeId, StateSeqNr};
    use crate::messaging::format::ProtocolMessageFormat;
    use crate::messaging::receiver::udp::UdpReceiver;
    use crate::messaging::{
        HelloMessage, PNetInterfaceMapper, ProtocolMessage, ProtocolMessageReceiver,
    };

    #[test]
    fn receive_timeout() -> Result<(), Box<dyn Error + Send + Sync>> {
        crate::tests::init();

        let ip_cache = Arc::new(RwLock::new(HashMap::new()));

        let mut receiver = UdpReceiver::new(
            0,
            ip_cache,
            PNetInterfaceMapper::new(),
            ProtocolMessageFormat::Json,
        )
        .expect("failed to create receiver");
        let addr = receiver.local_addr().expect("failed to get local addr");

        let handle = std::thread::spawn(move || {
            let socket = UdpSocket::bind("[::]:0").expect("failed to open socket");

            let protocol_message = ProtocolMessage::Hello(HelloMessage {
                source: NodeId::one(),
                source_state_seq_nr: StateSeqNr::from(0),
            });

            let mut buffer = Vec::new();

            ProtocolMessageFormat::Json
                .serialize(&mut buffer, &protocol_message)
                .expect("failed to serialize");

            socket
                .send_to(&buffer[..buffer.len()], addr)
                .expect("failed to send to address");
        });

        let received = receiver.recv_timeout(Some(Duration::from_micros(500)));

        handle.join().expect("failed to join");

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

    #[test]
    fn try_receive() -> Result<(), Box<dyn Error + Send + Sync>> {
        crate::tests::init();

        let ip_cache = Arc::new(RwLock::new(HashMap::new()));

        let mut receiver = UdpReceiver::new(
            0,
            ip_cache,
            PNetInterfaceMapper::new(),
            ProtocolMessageFormat::Json,
        )?;

        let addr = receiver.local_addr().expect("failed to get local addr");

        let handle = std::thread::spawn(move || {
            let socket = UdpSocket::bind("[::]:0")?;

            let protocol_message = ProtocolMessage::Hello(HelloMessage {
                source: NodeId::one(),
                source_state_seq_nr: StateSeqNr::from(0),
            });

            let mut buffer = Vec::new();

            ProtocolMessageFormat::Json.serialize(&mut buffer, &protocol_message)?;

            socket.send_to(&buffer[..buffer.len()], addr)?;

            Result::<(), Box<dyn Error + Send + Sync>>::Ok(())
        });

        handle.join().expect("failed to join")?;

        let received = receiver.try_recv();

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
