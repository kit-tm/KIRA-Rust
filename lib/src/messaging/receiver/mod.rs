use std::error::Error;
use std::fmt::{Debug, Display};
use std::time::Duration;

use crate::domain::NetworkInterface;
use crate::messaging::messages::ProtocolMessage;

#[cfg(feature = "udp-tokio")]
pub mod udp_tokio;

/// Error type for [ProtocolMessageReceiver::recv] and [AsyncProtocolMessageReceiver::recv].
#[derive(Debug)]
pub enum RecvError {
    /// Receiving timed out.
    Timeout,
    /// Returned if no interface for a message was found.
    ///
    /// This may signal an inconsistency in the interface configuration.
    NoInterfaceFound,
    /// Signals that no more messages will be received.
    Closed,
    /// The I/O-Layer returned some error.
    IoError(Box<dyn Error>),
    /// Other Error for custom error types of the implementations.
    Other(Box<dyn Error>),
}

impl Display for RecvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => write!(f, "Timeout receiving a Message"),
            Self::NoInterfaceFound => write!(
                f,
                "No interface for message was found; Network configuration may be inconsistent"
            ),
            Self::IoError(e) => write!(f, "Received IO Error: {}", e),
            Self::Other(e) => write!(f, "{}", e),
            Self::Closed => write!(f, "Receiver closed"),
        }
    }
}

impl Error for RecvError {}

/// Error type for [ProtocolMessageSender::try_recv] and [AsyncProtocolMessageSender::try_recv].
#[derive(Debug)]
pub enum TryRecvError {
    /// The I/O-Layer returned some error.
    IoError(Box<dyn Error>),
    /// Returned if no interface for a message was found.
    ///
    /// This may signal an inconsistency in the interface configuration.
    NoInterfaceFound,
    /// Signals that no more messages will be received.
    Closed,
    /// Other Error for custom error types of the implementations.
    Other(Box<dyn Error>),
}

impl Display for TryRecvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IoError(e) => write!(f, "Received IO Error: {}", e),
            Self::NoInterfaceFound => write!(
                f,
                "No interface for message was found; Network configuration may be inconsistent"
            ),
            Self::Other(e) => write!(f, "{}", e),
            Self::Closed => write!(f, "Receiver closed"),
        }
    }
}

impl Error for TryRecvError {}

/// Receives [Message]s of other Nodes.
///
/// Converts a [Message] formatted by its corresponding [MessageSender] back
/// to a [Message] and returns it.
///
/// Every method is allowed to return [None] at any point in time.
/// In cases where the [ProtocolMessageReceiver] is no longer able to receive messages
/// an error has to be returned.
pub trait ProtocolMessageReceiver: Debug {
    /// Receives a [Message].
    ///
    /// Returns an [Error] if receiving failed or the optional timeout was reached.
    ///
    /// If no timeout was given the operation waits until a new [Message] arrived.
    ///
    /// Returns [None] if no messages will be received from this [MessageReceiver] anymore.
    fn recv_timeout(
        &mut self,
        timeout: Option<Duration>,
    ) -> Result<Option<(ProtocolMessage, NetworkInterface)>, RecvError>;
    /// Receives a [Message].
    ///
    /// Short for calling [recv_timeout](MessageReceiver::recv_timeout) with [None](Option::None).
    fn recv(&mut self) -> Result<Option<(ProtocolMessage, NetworkInterface)>, RecvError> {
        self.recv_timeout(None)
    }
    /// Tries to receive a [Message] and returns an [Error] if no message is present at the time.
    fn try_recv(&mut self) -> Result<Option<(ProtocolMessage, NetworkInterface)>, TryRecvError>;
}

/// Receives [Message]s of other Nodes.
///
/// Converts a [Message] formatted by its corresponding [MessageSender] back
/// to a [Message] and returns it.
#[async_trait::async_trait]
pub trait AsyncProtocolMessageReceiver: Debug {
    /// Receives a [Message].
    ///
    /// Returns an [Error] if receiving failed or the optional timeout was reached.
    ///
    /// If no timeout was given the operation waits until a new [Message] arrived.
    ///
    /// Returns [None] if no messages will be received from this [MessageReceiver] anymore.
    async fn recv_timeout(
        &mut self,
        timeout: Option<Duration>,
    ) -> Result<Option<(ProtocolMessage, NetworkInterface)>, RecvError>;
    /// Receives a [Message].
    ///
    /// Short for calling [recv_timeout](MessageReceiver::recv_timeout) with [None](Option::None).
    async fn recv(&mut self) -> Result<Option<(ProtocolMessage, NetworkInterface)>, RecvError>;
    /// Tries to receive a [Message] and returns an [Error] if no message is present at the time.
    async fn try_recv(
        &mut self,
    ) -> Result<Option<(ProtocolMessage, NetworkInterface)>, TryRecvError>;
}
