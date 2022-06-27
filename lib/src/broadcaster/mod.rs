#[cfg(feature = "bus")]
pub use bus_broadcaster::*;
use std::fmt::Debug;
#[cfg(feature = "tokio")]
pub use tokio_broadcaster::*;

use crate::use_cases::UseCaseEvent;

#[cfg(feature = "bus")]
pub mod bus_broadcaster;
#[cfg(feature = "tokio")]
pub mod tokio_broadcaster;

pub trait Broadcaster: Clone {
    type SendError: Debug;
    type Subscriber;

    fn send_event(&self, event: UseCaseEvent) -> Result<(), Self::SendError>;
    fn subscribe(&self) -> Self::Subscriber;
}
