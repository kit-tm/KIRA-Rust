use std::ops::Deref;
use std::time::Duration;

#[cfg(feature = "tokio")]
pub use tokio_async_runtime::*;

use crate::usecases::broadcaster::Broadcaster;
use crate::usecases::UseCaseEvent;

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct TimerId(usize);

impl From<usize> for TimerId {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

impl Deref for TimerId {
    type Target = usize;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub trait Runtime {
    /// Either waits the duration instantly or returns and
    fn register_timer(&mut self, duration: Duration) -> TimerId;
}

/// Single Threaded Runtime using the standard library.
pub struct StdSyncRuntime<E, const ID_SIZE: usize> {
    id_counter: usize,
    broadcaster: E,
}

impl<E: Default, const ID_SIZE: usize> Default for StdSyncRuntime<E, ID_SIZE> {
    fn default() -> Self {
        Self {
            id_counter: 0,
            broadcaster: E::default(),
        }
    }
}

impl<E, const ID_SIZE: usize> StdSyncRuntime<E, ID_SIZE> {
    pub const fn new(executioner: E) -> Self {
        StdSyncRuntime {
            id_counter: 0,
            broadcaster: executioner,
        }
    }
}

impl<E: Broadcaster<ID_SIZE>, const ID_SIZE: usize> Runtime for StdSyncRuntime<E, ID_SIZE> {
    fn register_timer(&mut self, duration: Duration) -> TimerId {
        std::thread::sleep(duration);
        let timer_id = TimerId(self.id_counter);
        self.id_counter += 1;
        if let Err(e) = self.broadcaster.send_event(UseCaseEvent::Timer(timer_id)) {
            log::error!("Failed to send Event to use cases: {}", e);
        }
        timer_id
    }
}

#[cfg(feature = "tokio")]
mod tokio_async_runtime {
    use std::time::Duration;

    use crate::usecases::broadcaster::Broadcaster;
    use crate::usecases::{Runtime, TimerId, UseCaseEvent};

    pub struct AsyncTokioRuntime<E: Broadcaster<ID_SIZE>, const ID_SIZE: usize> {
        counter: usize,
        executioner: E,
    }

    impl<E: Broadcaster<ID_SIZE>, const ID_SIZE: usize> AsyncTokioRuntime<E, ID_SIZE> {
        pub const fn new(executioner: E) -> Self {
            Self {
                counter: 0,
                executioner,
            }
        }
    }

    impl<E: 'static + Broadcaster<ID_SIZE> + Send + Sync, const ID_SIZE: usize> Runtime
        for AsyncTokioRuntime<E, ID_SIZE>
    {
        fn register_timer(&mut self, duration: Duration) -> TimerId {
            let timer_id = TimerId::from(self.counter);
            self.counter += 1;

            let broadcaster = self.executioner.clone();
            tokio::spawn(async move {
                tokio::time::sleep(duration).await;
                if let Err(e) = broadcaster.send_event(UseCaseEvent::Timer(timer_id)) {
                    log::error!("Failed to send Event to use cases: {}", e);
                }
            });

            timer_id
        }
    }
}
