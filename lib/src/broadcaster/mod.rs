use std::error::Error;

#[cfg(feature = "tokio")]
pub use tokio_broadcaster::*;

use crate::usecases::UseCaseEvent;

#[cfg(feature = "tokio")]
pub mod tokio_broadcaster;

pub trait Broadcaster: Clone {
    type SendError: Error;
    type Subscriber;

    fn send_event(&self, event: UseCaseEvent) -> Result<(), Self::SendError>;
    fn subscribe(&self) -> Self::Subscriber;
}
