//! Interface definition and implementation for use case interaction with a runtime.

use std::fmt::Debug;
use std::time::Duration;

#[cfg(test)]
pub use immediate_runtime::*;
pub use sync_runtime::*;
#[cfg(feature = "tokio")]
pub use tokio_runtime::*;

use crate::use_cases::{TimerId, UseCaseEvent};

#[cfg(test)]
pub mod immediate_runtime;
pub mod sync_runtime;
#[cfg(feature = "tokio")]
pub mod tokio_runtime;

/// Interface for the [UseCases](crate::use_cases::UseCase) to the runtime environment.
pub trait UseCaseRuntime {
    type SendError: Debug;

    /// Creates a timer which will later yield a TimerEvent.
    ///
    /// The returned TimerId has to be unique.
    /// It's an error for runtimes to return duplicate [TimerId]s.
    fn register_timer(&self, duration: Duration) -> TimerId;

    /// Creates a periodic Timer.
    ///
    /// The returned TimerId has to be unique.
    /// It's an error for runtimes to return duplicate [TimerId]s.
    fn register_periodic_timer(&self, duration: Duration) -> TimerId;

    /// Broadcast a [UseCaseEvent] to all UseCases on the event-bus
    fn broadcast_local_event(&self, event: UseCaseEvent) -> Result<(), Self::SendError>;
}
