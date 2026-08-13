//! Traits and implementations for receiving [ProtocolMessages](kira_r2kad::domain::ProtocolMessage).

use std::{
    task::Poll,
    time::Duration,
};

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
/// A single [ProtocolMessageReceiver] can be responsible for one or many [InterfaceId]s.
///
/// Converts a [ProtocolMessage] formatted by its corresponding
/// [ProtocolMessageSender](super::sender::ProtocolMessageSender) back
/// to a [ProtocolMessage] and returns it.
///
/// Every method is allowed to return [None] at any point in time.
/// In cases where the [ProtocolMessageReceiver] is no longer able to receive messages
/// an error has to be returned.
pub trait ProtocolMessageReceiver {
    /// Receives a [ProtocolMessage].
    ///
    /// Returns an Error if receiving failed or the optional timeout was reached.
    ///
    /// If no timeout was given the operation waits until a new [ProtocolMessage] arrived.
    ///
    /// Returns [None] if no messages will be received from this [ProtocolMessageReceiver] anymore.
    fn recv_timeout(
        &mut self,
        timeout: Option<Duration>,
    ) -> Result<Option<(ProtocolMessage, UnderlayNeighborId)>, RecvError>;
    /// Receives a [ProtocolMessage].
    ///
    /// Short for calling [recv_timeout](ProtocolMessageReceiver::recv_timeout) with [None].
    fn recv(&mut self) -> Result<Option<(ProtocolMessage, UnderlayNeighborId)>, RecvError> {
        self.recv_timeout(None)
    }
    /// Tries to receive a [ProtocolMessage] and returns an Error if no message is present at the time.
    fn try_recv(&mut self) -> Result<Option<(ProtocolMessage, UnderlayNeighborId)>, TryRecvError>;
}

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
    use std::collections::HashSet;

    use derive_more::with_trait::{
        Display,
        Error,
    };
    use kira_r2kad::domain::InterfaceId;

    /// Error type for [AsyncProtocolMessageReceiver::recv](super::AsyncProtocolMessageReceiver::recv)
    #[derive(Debug, Display, Error)]
    pub enum RecvError {
        /// Receiving timed out.
        #[display("Timeout receiving a Message")]
        Timeout,
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
        IoError(Box<dyn Error + Send>),
        /// Other Error for custom error types of the implementations.
        #[display("Received IO Error: {_0}")]
        Other(Box<dyn Error + Send>),
    }

    /// Error type for [ProtocolMessageReceiver::try_recv](super::ProtocolMessageReceiver::try_recv).
    #[derive(Debug, Display, Error)]
    pub enum TryRecvError {
        /// The I/O-Layer returned some error.
        #[display("Received IO Error: {_0}")]
        IoError(Box<dyn Error + Send>),
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
        /// Other Error for custom error types of the implementations.
        Other(Box<dyn Error + Send>),
    }
}
