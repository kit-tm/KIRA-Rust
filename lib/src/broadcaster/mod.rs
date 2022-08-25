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
pub mod tokio_broadcaster;

/// A broadcaster handles sending events to the [UseCase]s or subscribing to these events.
///
/// Based on the execution model different broadcasters should be used:
///
/// - [BusBroadcaster] : When multiple threads send and receive events in a synchronous fashion.
/// - [TokioBroadcaster] : When multiple asynchronous tasks send and receive events.
pub trait Broadcaster: Clone {
    type SendError: Debug;
    type Subscriber;

    fn send_event(&self, event: UseCaseEvent) -> Result<(), Self::SendError>;
    fn subscribe(&self) -> Self::Subscriber;
}
