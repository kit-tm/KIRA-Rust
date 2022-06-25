use std::ops::DerefMut;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::runtime::Runtime;
use tokio::sync::RwLock;

use crate::domain::Port;
use crate::messaging::format::Format;
use crate::messaging::{Message, MessageReceiver, RecvTimeout, TryRecvError};

/// Maximum Transmission Unit (MTU). In general the MTU is actually smaller due to
/// network restrictions. But to be safe we use this.
const MTU_BYTES: usize = 65536;

/// The Buffer is not shared between instances of [UdpReceiver].
#[derive(Debug)]
pub struct UdpReceiver {
    buffer: RwLock<[u8; MTU_BYTES]>,
    socket: Arc<UdpSocket>,
    runtime: Arc<Runtime>,
    format: Format,
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
    pub fn new(socket: Arc<UdpSocket>, runtime: Arc<Runtime>, format: Format, port: Port) -> Self {
        Self {
            buffer: RwLock::new([0u8; MTU_BYTES]),
            socket,
            runtime,
            format,
            port,
        }
    }

    fn deserialize(&self, buffer: &[u8]) -> Option<(Message, Port)> {
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

impl<'a> MessageReceiver for &'a mut UdpReceiver {
    fn recv_timeout(
        &mut self,
        timeout: Option<Duration>,
    ) -> Result<Option<(Message, Port)>, RecvTimeout> {
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

    fn try_recv(&mut self) -> Result<Option<(Message, Port)>, TryRecvError> {
        let mut buffer = self.buffer.blocking_write();
        let received = match self.socket.try_recv(buffer.deref_mut()) {
            Ok(received) => received,
            Err(_) => return Err(TryRecvError),
        };

        Ok(self.deserialize(&buffer[..received]))
    }
}
