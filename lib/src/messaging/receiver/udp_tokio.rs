use std::ops::DerefMut;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::runtime::Runtime;
use tokio::sync::RwLock;

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
    runtime: Arc<Runtime>,
    format: ProtocolMessageFormat,
    port: Port,
}

impl Clone for UdpReceiver {
    /// Clones the [UdpReceiver] with a new buffer.
    fn clone(&self) -> Self {
        Self {
            buffer: RwLock::new([0u8; MTU_BYTES]),
            socket: Arc::clone(&self.socket),
            runtime: Arc::clone(&self.runtime),
            format: self.format.clone(),
            port: self.port.clone(),
        }
    }
}

impl UdpReceiver {
    pub fn new(
        socket: Arc<UdpSocket>,
        runtime: Arc<Runtime>,
        format: ProtocolMessageFormat,
        port: Port,
    ) -> Self {
        Self {
            buffer: RwLock::new([0u8; MTU_BYTES]),
            socket,
            runtime,
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
        timeout: Option<Duration>,
    ) -> Result<Option<(ProtocolMessage, Port)>, RecvTimeout> {
        let socket = Arc::clone(&self.socket);
        let mut buffer = self.buffer.blocking_write();

        let receive_with_optional_timeout = self.runtime.block_on(async move {
            if let Some(duration) = timeout {
                tokio::time::timeout(duration, socket.recv(buffer.deref_mut()))
                    .await
                    .map_err(|_| RecvTimeout)
            } else {
                Ok(socket.recv(buffer.deref_mut()).await)
            }
        })?;

        let received = match receive_with_optional_timeout {
            Ok(bytes) => bytes,
            Err(e) => {
                log::error!("Failed to receive data from socket: {}", e);
                return Ok(None);
            }
        };

        let buffer = self.buffer.blocking_read();
        Ok(self.deserialize(&buffer[..received]))
    }

    fn try_recv(&mut self) -> Result<Option<(ProtocolMessage, Port)>, TryRecvError> {
        let mut buffer = self.buffer.blocking_write();
        let received = match self.socket.try_recv(buffer.deref_mut()) {
            Ok(received) => received,
            Err(_) => return Err(TryRecvError),
        };

        Ok(self.deserialize(&buffer[..received]))
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::net::UdpSocket;
    use tokio::runtime::Builder;

    use crate::domain::{NodeId, Port, StateSeqNr};
    use crate::messaging::format::ProtocolMessageFormat;
    use crate::messaging::udp_tokio::UdpReceiver;
    use crate::messaging::{HelloMessage, ProtocolMessage, ProtocolMessageReceiver};

    #[test]
    fn receive_timeout() -> Result<(), Box<dyn Error + Send + Sync>> {
        crate::tests::init();

        let runtime = Builder::new_current_thread().enable_all().build()?;
        let runtime = Arc::new(runtime);

        let port = Port::new(String::from("test"));
        let socket = Arc::new(runtime.block_on(UdpSocket::bind("0.0.0.0:0"))?);
        let addr = socket.local_addr()?;

        let mut receiver = UdpReceiver::new(
            socket,
            runtime.clone(),
            ProtocolMessageFormat::Json,
            port.clone(),
        );

        let handle = runtime.spawn(async move {
            let socket = UdpSocket::bind("0.0.0.0:0").await?;

            let protocol_message = ProtocolMessage::Hello(HelloMessage {
                source: NodeId::one(),
                source_state_seq_nr: StateSeqNr::from(0),
            });

            let mut buffer = Vec::new();

            ProtocolMessageFormat::Json.serialize(&mut buffer, &protocol_message)?;

            socket.send_to(&buffer[..buffer.len()], addr).await?;

            Result::<(), Box<dyn Error + Send + Sync>>::Ok(())
        });

        let received = receiver.recv_timeout(Some(Duration::from_micros(500)));

        runtime.block_on(handle).expect("failed to join")?;

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

        let runtime = Builder::new_current_thread().enable_all().build()?;
        let runtime = Arc::new(runtime);

        let port = Port::new(String::from("test"));
        let socket = Arc::new(runtime.block_on(UdpSocket::bind("0.0.0.0:0"))?);
        let addr = socket.local_addr()?;

        let mut receiver = UdpReceiver::new(
            socket,
            runtime.clone(),
            ProtocolMessageFormat::Json,
            port.clone(),
        );

        let handle = runtime.spawn(async move {
            let socket = UdpSocket::bind("0.0.0.0:0").await?;

            let protocol_message = ProtocolMessage::Hello(HelloMessage {
                source: NodeId::one(),
                source_state_seq_nr: StateSeqNr::from(0),
            });

            let mut buffer = Vec::new();

            ProtocolMessageFormat::Json.serialize(&mut buffer, &protocol_message)?;

            socket.send_to(&buffer[..buffer.len()], addr).await?;

            Result::<(), Box<dyn Error + Send + Sync>>::Ok(())
        });

        runtime.block_on(handle).expect("failed to join")?;

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
