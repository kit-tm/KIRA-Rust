//! Used to broadcast [UseCaseEvents](UseCaseEvent) to all [UseCases](crate::use_cases::UseCase).
//!
//! Also adds implementations for tokio based channels like [tokio::sync::broadcast::Sender] and [tokio::sync::mpsc::UnboundedSender].

use std::fmt::Debug;

#[cfg(any(test, feature = "bus"))]
pub use bus_broadcaster::*;
pub use mpsc_broadcaster::*;
#[cfg(feature = "tokio")]
pub use tokio_broadcaster::*;

use crate::use_cases::UseCaseEvent;

#[cfg(any(test, feature = "bus"))]
pub mod bus_broadcaster;
pub mod mpsc_broadcaster;
#[cfg(feature = "tokio")]
mod tokio_broadcaster;

/// A broadcaster handles sending events to the [UseCases](crate::use_cases::UseCase) or subscribing to these events.
///
/// Based on the execution model different broadcasters should be used:
///
/// - [BusBroadcaster](BusBroadcaster) : When multiple threads send and receive events in a synchronous fashion.
/// - [Broadcast Sender](tokio::sync::broadcast::Sender) or [MPSC UnboundedSender](tokio::sync::mpsc::UnboundedSender) : When multiple asynchronous tasks send and receive events.
pub trait Broadcaster: Clone {
    type SendError: Debug;
    type Subscriber;

    fn send_event(&self, event: UseCaseEvent) -> Result<(), Self::SendError>;
    fn subscribe(&self) -> Self::Subscriber;
}
