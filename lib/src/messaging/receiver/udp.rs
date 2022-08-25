use std::io::ErrorKind;
use std::net::UdpSocket;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::domain::Port;
use crate::messaging::format::ProtocolMessageFormat;
use crate::messaging::{ProtocolMessage, ProtocolMessageReceiver, RecvTimeout, TryRecvError};

/// Maximum Transmission Unit (MTU). In general the MTU is actually smaller due to
/// network restrictions. But to be safe we use this.
const MTU_BYTES: usize = 65536;

/// The Buffer is not shared between instances of [UdpReceiver].
#[derive(Debug)]
pub struct UdpReceiver {
    buffer: RwLock<[u8; MTU_BYTES]>,
    socket: Arc<UdpSocket>,
    format: ProtocolMessageFormat,
    port: Port,
}

impl Clone for UdpReceiver {
    /// Clones the [UdpReceiver] with a new buffer.
    fn clone(&self) -> Self {
        Self {
            buffer: RwLock::new([0u8; MTU_BYTES]),
            socket: Arc::clone(&self.socket),
            format: self.format.clone(),
            port: self.port.clone(),
        }
    }
}

impl UdpReceiver {
    pub fn new(socket: Arc<UdpSocket>, format: ProtocolMessageFormat, port: Port) -> Self {
        Self {
            buffer: RwLock::new([0u8; MTU_BYTES]),
            socket,
            format,
            port,
        }
    }

    fn deserialize(&self, buffer: &[u8]) -> Option<(ProtocolMessage, Port)> {
        let deserialized = match self.format.deserialize(buffer) {
            Ok(message) => message,
            Err(e) => {
                log::debug!("Received invalid serialized message: {}", e);
                return None;
            }
        };

        Some((deserialized, self.port.clone()))
    }
}

impl ProtocolMessageReceiver for UdpReceiver {
    fn recv_timeout(
        &mut self,
        mut timeout: Option<Duration>,
    ) -> Result<Option<(ProtocolMessage, Port)>, RecvTimeout> {
        let socket = Arc::clone(&self.socket);
        let mut buffer = self.buffer.write().expect("failed to get write lock");

        timeout = timeout.and_then(|dur| if dur.is_zero() { None } else { Some(dur) });
        let read_timeout_before = socket.read_timeout().expect("failed to get read timeout");
        socket
            .set_read_timeout(timeout)
            .expect("invalid timeout set"); // Shouldn't happen due to previous check

        let receive_with_optional_timeout = socket.recv(&mut buffer[..]);

        // Reset settings after receive
        if let Err(e) = socket.set_read_timeout(read_timeout_before) {
            log::error!("Failed to reset read timeout: {}", e);
        }

        let received = match receive_with_optional_timeout {
            Ok(bytes) => bytes,
            Err(e) => {
                return match e.kind() {
                    ErrorKind::WouldBlock | ErrorKind::TimedOut => Err(RecvTimeout),
                    _ => {
                        log::error!("Failed to receive message from udp: {}", e);
                        Ok(None)
                    }
                };
            }
        };

        Ok(self.deserialize(&buffer[..received]))
    }

    fn try_recv(&mut self) -> Result<Option<(ProtocolMessage, Port)>, TryRecvError> {
        let mut buffer = self
            .buffer
            .write()
            .expect("failed to get write lock on buffer");

        self.socket
            .set_nonblocking(true)
            .expect("failed to set udp socket to nonblocking");

        let received = match self.socket.recv(&mut buffer[..]) {
            Ok(received) => received,
            Err(_) => return Err(TryRecvError),
        };

        self.socket
            .set_nonblocking(false)
            .expect("failed to set udp socket to blocking");

        Ok(self.deserialize(&buffer[..received]))
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::net::UdpSocket;
    use std::sync::Arc;
    use std::time::Duration;

    use crate::domain::{NodeId, Port, StateSeqNr};
    use crate::messaging::format::ProtocolMessageFormat;
    use crate::messaging::{HelloMessage, ProtocolMessage, ProtocolMessageReceiver};
    use crate::messaging::udp::UdpReceiver;

    #[test]
    fn receive_timeout() -> Result<(), Box<dyn Error + Send + Sync>> {
        crate::tests::init();

        let port = Port::new(String::from("test"));
        let socket = Arc::new(UdpSocket::bind("0.0.0.0:0")?);
        let addr = socket.local_addr()?;

        let mut receiver = UdpReceiver::new(socket, ProtocolMessageFormat::Json, port.clone());

        let handle = std::thread::spawn(move || {
            let socket = UdpSocket::bind("0.0.0.0:0")?;

            let protocol_message = ProtocolMessage::Hello(HelloMessage {
                source: NodeId::one(),
                source_state_seq_nr: StateSeqNr::from(0),
            });

            let mut buffer = Vec::new();

            ProtocolMessageFormat::Json.serialize(&mut buffer, &protocol_message)?;

            socket.send_to(&buffer[..buffer.len()], addr)?;

            Result::<(), Box<dyn Error + Send + Sync>>::Ok(())
        });

        let received = receiver.recv_timeout(Some(Duration::from_micros(500)));

        handle.join().expect("failed to join")?;

        assert!(received.is_ok(), "{:?}", received);
        let received = received.unwrap();
        assert!(received.is_some(), "{:?}", received);
        let (message, source) = received.unwrap();
        assert_eq!(source, port);
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

        let port = Port::new(String::from("test"));
        let socket = Arc::new(UdpSocket::bind("0.0.0.0:0")?);
        let addr = socket.local_addr()?;

        let mut receiver = UdpReceiver::new(socket, ProtocolMessageFormat::Json, port.clone());

        let handle = std::thread::spawn(move || {
            let socket = UdpSocket::bind("0.0.0.0:0")?;

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
        let (message, source) = received.unwrap();
        assert_eq!(source, port);
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
