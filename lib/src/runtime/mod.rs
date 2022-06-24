use std::time::Duration;

pub use sync_runtime::*;
#[cfg(feature = "tokio")]
pub use tokio_runtime::*;

use crate::usecases::TimerId;

pub mod sync_runtime;
#[cfg(feature = "tokio")]
pub mod tokio_runtime;

//Additionally: Consider "wait for message" to reduce amount of woken up UseCases
pub trait Runtime {
    /// Either waits the duration instantly or returns and
    fn register_timer(&self, duration: Duration) -> TimerId;
}
