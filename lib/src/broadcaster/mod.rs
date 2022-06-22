use std::error::Error;

#[cfg(feature = "tokio")]
pub use tokio_broadcaster::*;

use crate::usecases::UseCaseEvent;

#[cfg(feature = "tokio")]
pub mod tokio_broadcaster;

pub trait Broadcaster<const ID_SIZE: usize>: Clone {
    type SendError: Error;
    type Subscriber;

    fn send_event(&self, event: UseCaseEvent<ID_SIZE>) -> Result<(), Self::SendError>;
    fn subscribe(&self) -> Self::Subscriber;
}
