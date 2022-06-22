use crate::messaging::messages::Message;
use std::error::Error;

/// Sends [Message]s to other Nodes.
///
/// Derives how and where to send the [Message] by analyzing its fields
/// and converts them to an appropriate format so that the corresponding
/// [MessageReceiver] can convert it back to a [Message].
pub trait MessageSender<const ID_SIZE: usize> {
    type Error: Error;

    /// Sends a [Message] to another Node, converting it to an appropriate
    /// format before sending.
    ///
    /// Returns an Error if the operation or formatting failed.
    fn send<M>(&mut self, message: M) -> Result<(), Self::Error>
    where
        M: Into<Message<ID_SIZE>>;
}
