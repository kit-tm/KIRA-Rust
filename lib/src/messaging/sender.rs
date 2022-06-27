use std::error::Error;

use crate::messaging::messages::ProtocolMessage;

/// Sends [Message]s to other Nodes.
///
/// Derives how and where to send the [Message] by analyzing its fields
/// and converts them to an appropriate format so that the corresponding
/// [MessageReceiver] can convert it back to a [Message].
///
/// # Hello Messages
///
/// [HelloMessage]s with a [NodeId::zero] destination have to be broadcast to all [Port]s.
pub trait ProtocolMessageSender {
    type Error: Error;

    /// Sends a [Message] to another Node, converting it to an appropriate
    /// format before sending.
    ///
    /// Returns an Error if the operation or formatting failed.
    fn send<M>(&mut self, message: M) -> Result<(), Self::Error>
    where
        M: Into<ProtocolMessage>;
}
