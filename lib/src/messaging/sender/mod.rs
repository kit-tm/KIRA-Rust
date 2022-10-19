use crate::messaging::messages::ProtocolMessage;

#[cfg(feature = "udp-tokio")]
pub mod udp_tokio;

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
    /// Sends a [Message] to another Node, converting it to an appropriate
    /// format before sending.
    ///
    /// Returns an Error if the operation or formatting failed.
    fn send_message<M>(&mut self, message: M) -> Result<(), error::SenderError>
    where
        M: Into<ProtocolMessage>;
}

/// Sends [Message]s to other Nodes.
///
/// Derives how and where to send the [Message] by analyzing its fields
/// and converts them to an appropriate format so that the corresponding
/// [MessageReceiver] can convert it back to a [Message].
///
/// # Hello Messages
///
/// [HelloMessage]s with a [NodeId::zero] destination have to be broadcast to all [Port]s.
#[async_trait::async_trait]
pub trait AsyncProtocolMessageSender {
    /// Sends a [Message] to another Node, converting it to an appropriate
    /// format before sending.
    ///
    /// Returns an Error if the operation or formatting failed.
    async fn send_message<M>(&mut self, message: M) -> Result<(), error::SenderError>
    where
        M: Into<ProtocolMessage> + Send + Sync;
}

pub mod error {
    use std::error::Error;
    use std::fmt::{Display, Formatter};
    use std::io;

    /// Error type for [ProtocolMessageSender] and [AsyncProtocolMessageSender].
    #[derive(Debug)]
    pub enum SenderError {
        /// Error while deserializing message.
        MessageFormat(Box<dyn Error + Sync + Send>),
        /// [io::Error] while sending a message occurred.
        SendError(io::Error),
        /// Wrapper for other errors to support custom types for individual implementations.
        Other(Box<dyn Error + Sync + Send>),
        /// No more messages can be sent on this sender.
        Closed,
    }

    impl Display for SenderError {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::MessageFormat(e) => write!(f, "Failed to serialize message: {}", e),
                Self::SendError(e) => write!(f, "Sending failed: {}", e),
                Self::Other(e) => write!(f, "{}", e),
                Self::Closed => write!(f, "Sender closed"),
            }
        }
    }

    impl Error for SenderError {}

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
