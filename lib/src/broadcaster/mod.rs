#[cfg(feature = "bus")]
pub use bus::*;
use std::fmt::Debug;
#[cfg(feature = "tokio")]
pub use tokio::*;

use crate::use_cases::UseCaseEvent;

#[cfg(feature = "bus")]
pub mod bus;
#[cfg(feature = "tokio")]
pub mod tokio;

pub trait Broadcaster: Clone {
    type SendError: Debug;
    type Subscriber;

    fn send_event(&self, event: UseCaseEvent) -> Result<(), Self::SendError>;
    fn subscribe(&self) -> Self::Subscriber;
}
