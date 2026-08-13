//! Traits and implementations for sending [ProtocolMessages](kira_r2kad::messaging::ProtocolMessage).

use error::*;
pub use kira_r2kad::{
    domain::UnderlayNeighborDestination,
    messaging::messages::ProtocolMessage,
};

#[cfg(feature = "udp-tokio")]
pub mod udp_tokio;

/// Sends [ProtocolMessage]s to other Nodes.
///
/// Derives how and where to send the [ProtocolMessage] by analyzing its fields
/// and converts them to an appropriate format so that the corresponding
/// [ProtocolMessageReceiver](super::receiver::ProtocolMessageReceiver)
/// can convert it back to a [ProtocolMessage].
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
/// [ProtocolMessageReceiver](super::receiver::ProtocolMessageReceiver)
/// can convert it back to a [ProtocolMessage].
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
        M: Into<ProtocolMessage> + Send + Sync + std::fmt::Debug;
}

/// Errors for message senders.
pub mod error {
    use std::{
        error::Error,
        io,
    };

    use derive_more::derive::{
        Display,
        Error,
    };

    /// Error type for [AsyncProtocolMessageSender](super::AsyncProtocolMessageSender).
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
