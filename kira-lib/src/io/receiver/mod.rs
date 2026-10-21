//! Traits and implementations for receiving [ProtocolMessages](kira_r2kad::domain::ProtocolMessage).

use std::task::Poll;

use error::*;
use futures::{
    FutureExt,
    Stream,
};
pub use kira_r2kad::domain::{
    InterfaceId,
    ProtocolMessage,
    UnderlayNeighborId,
};

#[cfg(feature = "udp-tokio")]
pub mod udp_tokio;

/// Receives [ProtocolMessage]s of other Nodes.
///
/// Converts a [ProtocolMessage] formatted by its corresponding
/// [ProtocolMessageSender](super::sender::ProtocolMessageSender) back
/// to a [ProtocolMessage] and returns it.
#[trait_variant::make(AsyncProtocolMessageReceiver: Send)]
pub trait LocalAsyncProtocolMessageReceiver {
    /// Receives a [ProtocolMessage].
    ///
    /// Waits until a new [ProtocolMessage] arrived.
    /// Returns an Error if receiving failed.
    /// Returns [None] if no messages will be received from this [ProtocolMessageReceiver] anymore.
    async fn recv(&mut self) -> Option<Result<(ProtocolMessage, UnderlayNeighborId), RecvError>>;
}

/// Struct wrapping an [AsyncProtocolMessageReceiver] for implementing [Stream] and
/// [TryStream](futures::prelude::TryStream).
pub struct ProtocolMessageReceiverStream<R>(R);

impl<R: AsyncProtocolMessageReceiver + Unpin> Stream for ProtocolMessageReceiverStream<R> {
    type Item = Result<(ProtocolMessage, UnderlayNeighborId), RecvError>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let Self(rx) = self.get_mut();
        let mut rx = Box::pin(rx.recv());
        rx.poll_unpin(cx)
    }
}

/// Errors for message receivers
pub mod error {
    use std::{
        collections::HashSet,
        io,
    };

    use derive_more::with_trait::{
        Display,
        Error,
    };
    use kira_r2kad::domain::InterfaceId;

    /// Error type for [AsyncProtocolMessageReceiver::recv](super::AsyncProtocolMessageReceiver::recv)
    #[derive(Debug, Display, Error)]
    pub enum RecvError {
        /// Returned if no interface for a message was found.
        ///
        /// This may signal an inconsistency in the interface configuration.
        #[display("No interface for message was found; Network configuration may be inconsistent")]
        NoInterfaceFound,
        /// Signals that one or many interfaces stopped working.
        ///
        /// This doesn't signal that the receiver stops operating.
        #[display("Interfaces {_0:?} stopped working")]
        InterfacesDown(#[error(ignore)] HashSet<InterfaceId>),
        /// Signals that no more messages will be received from this receiver.
        #[display("Receiver closed")]
        Closed,
        /// The I/O-Layer returned some error.
        IoError(io::Error),
        /// Other Error for custom error types of the implementations.
        #[display("Received IO Error: {_0}")]
        Other(Box<dyn Error + Send>),
    }

    impl From<io::Error> for RecvError {
        fn from(err: io::Error) -> Self {
            RecvError::IoError(err)
        }
    }
}
