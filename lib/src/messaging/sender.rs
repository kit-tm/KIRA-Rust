use crate::messaging::messages::ProtocolMessage;
use std::error::Error;

/// Sends [Message]s to other Nodes.
///
/// Derives how and where to send the [Message] by analyzing its fields
/// and converts them to an appropriate format so that the corresponding
/// [MessageReceiver] can convert it back to a [Message].
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
