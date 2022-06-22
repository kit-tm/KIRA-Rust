use crate::domain::Interface;
use crate::messaging::messages::Message;
use std::error::Error;
use std::fmt::Display;
use std::time::Duration;

#[derive(Debug)]
pub struct RecvTimeout;

impl Display for RecvTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Timeout receiving a Message")
    }
}

impl Error for RecvTimeout {}

#[derive(Debug)]
pub struct TryRecvError;

impl Display for TryRecvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Timeout receiving a Message")
    }
}

impl Error for TryRecvError {}

/// Receives [Message]s of other Nodes.
///
/// Converts a [Message] formatted by its corresponding [MessageSender] back
/// to a [Message] and returns it.
pub trait MessageReceiver<const ID_SIZE: usize> {
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
    ) -> Result<Option<(Message<ID_SIZE>, Interface)>, RecvTimeout>;
    /// Receives a [Message].
    ///
    /// Short for calling [recv_timeout](MessageReceiver::recv_timeout) with [None](Option::None).
    fn recv(&mut self) -> Option<(Message<ID_SIZE>, Interface)> {
        self.recv_timeout(None).ok().flatten()
    }
    /// Tries to receive a [Message] and returns an [Error] if no message is present at the time.
    fn try_recv(&mut self) -> Result<Option<(Message<ID_SIZE>, Interface)>, TryRecvError>;
}
