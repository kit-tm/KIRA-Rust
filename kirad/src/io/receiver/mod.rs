//! Traits and implementations for receiving [ProtocolMessages](kira_lib::messaging::ProtocolMessage).

use std::task::Poll;
use std::time::Duration;

use error::*;

use futures::FutureExt;
use futures::Stream;
pub use kira_lib::domain::InterfaceId;
pub use kira_lib::domain::UnderlayNeighborId;
pub use kira_lib::messaging::messages::ProtocolMessage;

#[cfg(feature = "udp-tokio")]
pub mod udp_tokio;

/// Receives [ProtocolMessage]s of other Nodes.
///
/// A single [ProtocolMessageReceiver] can be responsible for one or many [InterfaceId]s.
///
/// Converts a [ProtocolMessage] formatted by its corresponding
/// [ProtocolMessageSender](crate::messaging::sender::ProtocolMessageSender) back
/// to a [ProtocolMessage] and returns it.
///
/// Every method is allowed to return [None] at any point in time.
/// In cases where the [ProtocolMessageReceiver] is no longer able to receive messages
/// an error has to be returned.
pub trait ProtocolMessageReceiver {
    /// Receives a [ProtocolMessage].
    ///
    /// Returns an [Error] if receiving failed or the optional timeout was reached.
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
/// [ProtocolMessageSender](crate::messaging::sender::ProtocolMessageSender) back
/// to a [ProtocolMessage] and returns it.
#[trait_variant::make(AsyncProtocolMessageReceiver: Send)]
pub trait LocalAsyncProtocolMessageReceiver {
    /// Receives a [ProtocolMessage].
    ///
    /// Returns an [Error] if receiving failed or the optional timeout was reached.
    ///
    /// If no timeout was given the operation waits until a new [ProtocolMessage] arrived.
    ///
    /// Returns [None] if no messages will be received from this [ProtocolMessageReceiver] anymore.
    async fn recv_timeout(
        &mut self,
        timeout: Option<Duration>,
    ) -> Result<Option<(ProtocolMessage, UnderlayNeighborId)>, RecvError>;
    /// Receives a [ProtocolMessage].
    ///
    /// Short for calling [recv_timeout](ProtocolMessageReceiver::recv_timeout) with
    /// [None].
    async fn recv(&mut self) -> Result<Option<(ProtocolMessage, UnderlayNeighborId)>, RecvError>;
    /// Tries to receive a [ProtocolMessage] and returns an [Error] if no message is present at the time.
    async fn try_recv(
        &mut self,
    ) -> Result<Option<(ProtocolMessage, UnderlayNeighborId)>, TryRecvError>;
}

/// Struct wrapping an [AsyncProtocolMessageReceiver] for implementing [Stream] and [TryStream].
pub struct ProtocolMessageReceiverStream<R>(R);

impl<R: AsyncProtocolMessageReceiver + Unpin> Stream for ProtocolMessageReceiverStream<R> {
    type Item = Result<(ProtocolMessage, UnderlayNeighborId), RecvError>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        let Self(rx) = self.get_mut();
        let mut rx = Box::pin(rx.recv());

        match rx.poll_unpin(cx) {
            Poll::Ready(Ok(Some(item))) => Poll::Ready(Some(Ok(item))),
            Poll::Ready(Err(e)) => Poll::Ready(Some(Err(e))),
            Poll::Ready(Ok(None)) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Errors for message receivers
pub mod error {
    use std::collections::HashSet;

    use derive_more::{Display, Error};
    use kira_lib::domain::InterfaceId;

    /// Error type for [ProtocolMessageReceiver::recv](super::ProtocolMessageReceiver::recv)
    /// and [AsyncProtocolMessageReceiver::recv](super::AsyncProtocolMessageReceiver::recv).
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
        ///
        /// Also includes the remaining interfaces this receiver handled.
        #[display("Receiver for interfaces {_0:?} closed")]
        Closed(#[error(ignore)] HashSet<InterfaceId>),
        /// The I/O-Layer returned some error.
        IoError(Box<dyn Error + Send>),
        /// Other Error for custom error types of the implementations.
        #[display("Received IO Error: {_0}")]
        Other(Box<dyn Error + Send>),
    }

    /// Error type for [ProtocolMessageReceiver::try_recv](super::ProtocolMessageReceiver::try_recv) and
    /// [ProtocolMessageReceiver::try_recv](super::ProtocolMessageReceiver::try_recv).
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
        ///
        /// Also includes the remaining interfaces this receiver handled.
        #[display("Receiver for interfaces {_0:?} closed")]
        Closed(#[error(ignore)] HashSet<InterfaceId>),
        /// Other Error for custom error types of the implementations.
        Other(Box<dyn Error + Send>),
    }
}
