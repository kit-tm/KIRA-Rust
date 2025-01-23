//! Traits and implementations for sending [ProtocolMessages](kira_lib::messaging::ProtocolMessage).

use error::*;

use futures::Sink;
pub use kira_lib::domain::UnderlayNeighborDestination;
pub use kira_lib::messaging::messages::ProtocolMessage;

#[cfg(feature = "udp-tokio")]
pub mod udp_tokio;

/// Sends [ProtocolMessage]s to other Nodes.
///
/// Derives how and where to send the [ProtocolMessage] by analyzing its fields
/// and converts them to an appropriate format so that the corresponding
/// [ProtocolMessageReceiver](crate::messaging::receiver::ProtocolMessageReceiver)
/// can convert it back to a [ProtocolMessage].
///
/// # Hello Messages
///
/// [ProtocolMessage::Hello]s with a [NodeId::zero](crate::domain::NodeId::zero) destination have
/// to be broadcast to all [NetworkInterfaces](crate::domain::NetworkInterface).
pub trait ProtocolMessageSender {
    /// Sends a [ProtocolMessage] to another Node, converting it to an appropriate
    /// format before sending.
    ///
    /// Returns an Error if the operation or formatting failed.
    fn send_message<M>(
        &mut self,
        message: M,
        destination: UnderlayNeighborDestination,
    ) -> Result<(), SenderError>
    where
        M: Into<ProtocolMessage>;
}

/// Sends [ProtocolMessage]s to other Nodes.
///
/// Derives how and where to send the [ProtocolMessage] by analyzing its fields
/// and converts them to an appropriate format so that the corresponding
/// [ProtocolMessageReceiver](crate::messaging::receiver::ProtocolMessageReceiver)
/// can convert it back to a [ProtocolMessage].
///
/// # Hello Messages
///
/// [ProtocolMessage::Hello]s with a [NodeId::zero](crate::domain::NodeId::zero) destination have
/// to be broadcast to all [NetworkInterfaces](crate::domain::NetworkInterface).
#[trait_variant::make(AsyncProtocolMessageSender: Send)]
pub trait LocalAsyncProtocolMessageSender {
    /// Sends a [ProtocolMessage] to another Node, converting it to an appropriate
    /// format before sending.
    ///
    /// Returns an Error if the operation or formatting failed.
    async fn send_message<M>(
        &mut self,
        message: M,
        destination: UnderlayNeighborDestination,
    ) -> Result<(), SenderError>
    where
        M: Into<ProtocolMessage> + Send + Sync;
}

/// Errors for message senders.
pub mod error {
    use derive_more::derive::{Display, Error};
    use std::error::Error;
    use std::io;

    /// Error type for [ProtocolMessageSender](crate::messaging::sender::ProtocolMessageSender) and
    /// [AsyncProtocolMessageSender](crate::messaging::sender::AsyncProtocolMessageSender).
    #[derive(Debug, Display, Error)]
    pub enum SenderError {
        /// Error while deserializing message.
        #[display("Failed to serialize message: {_0}")]
        MessageFormat(Box<dyn Error + Sync + Send>),
        /// [io::Error] while sending a message occurred.
        #[display("Sending failed: {_0:?}")]
        SendError(io::Error),
        /// Wrapper for other errors to support custom types for individual implementations.
        Other(Box<dyn Error + Sync + Send>),
        /// No more messages can be sent on this sender.
        #[display("Sender closed")]
        Closed,
    }

    impl From<Box<dyn Error + Sync + Send>> for SenderError {
        fn from(value: Box<dyn Error + Sync + Send>) -> Self {
            Self::MessageFormat(value)
        }
    }

    impl From<io::Error> for SenderError {
        fn from(io_err: io::Error) -> Self {
            Self::SendError(io_err)
        }
    }
}
