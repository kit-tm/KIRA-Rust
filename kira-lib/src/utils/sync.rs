//! Generic interfaces and implementations of sync primitives.

use std::error::Error;

/// Generic trait to send data to a generic destination.
///
/// This trait is heavily inspired by [Sender](std::sync::mpsc::Sender).
pub trait Sender<T> {
    type SenderError: Error;

    /// Sends a value `T` to a destination.
    ///
    /// # Note
    ///
    /// This function requires mutability since in the only current use
    /// there is only a single producer: [Runtime](crate::runtime::UseCaseRuntime).
    fn send(&mut self, value: T) -> Result<(), Self::SenderError>;
}

impl<T> Sender<T> for std::sync::mpsc::Sender<T> {
    type SenderError = std::sync::mpsc::SendError<T>;

    fn send(&mut self, value: T) -> Result<(), Self::SenderError> {
        std::sync::mpsc::Sender::<T>::send(self, value)
    }
}

#[cfg(feature = "tokio")]
impl<T> Sender<T> for tokio::sync::mpsc::Sender<T> {
    type SenderError = tokio::sync::mpsc::error::SendError<T>;

    fn send(&mut self, value: T) -> Result<(), Self::SenderError> {
        self.blocking_send(value)
    }
}
