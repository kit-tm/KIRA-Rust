use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

#[cfg(feature = "tokio")]
pub use tokio_async_runtime::*;

use crate::broadcaster::Broadcaster;
use crate::usecases::{TimerId, UseCaseEvent};

pub trait Runtime {
    /// Either waits the duration instantly or returns and
    fn register_timer(&self, duration: Duration) -> TimerId;
}

/// Single Threaded Runtime using the standard library.
pub struct StdSyncRuntime<E, const ID_SIZE: usize> {
    id_counter: AtomicUsize,
    broadcaster: E,
}

impl<E: Default, const ID_SIZE: usize> Default for StdSyncRuntime<E, ID_SIZE> {
    fn default() -> Self {
        Self {
            id_counter: AtomicUsize::new(0),
            broadcaster: E::default(),
        }
    }
}

impl<E, const ID_SIZE: usize> StdSyncRuntime<E, ID_SIZE> {
    pub const fn new(executioner: E) -> Self {
        StdSyncRuntime {
            id_counter: AtomicUsize::new(0),
            broadcaster: executioner,
        }
    }
}

impl<E: Broadcaster<ID_SIZE>, const ID_SIZE: usize> Runtime for StdSyncRuntime<E, ID_SIZE> {
    fn register_timer(&self, duration: Duration) -> TimerId {
        std::thread::sleep(duration);
        let timer_id = TimerId::from(self.id_counter.fetch_add(1, Ordering::Relaxed));
        if let Err(e) = self.broadcaster.send_event(UseCaseEvent::Timer(timer_id)) {
            log::error!("Failed to send Event to use cases: {}", e);
        }
        timer_id
    }
}

#[cfg(feature = "tokio")]
mod tokio_async_runtime {
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::sync::Mutex;

    use crate::broadcaster::Broadcaster;
    use crate::runtime::{Runtime, TimerId};
    use crate::usecases::UseCaseEvent;

    pub struct AsyncTokioRuntime<B: Broadcaster<ID_SIZE>, const ID_SIZE: usize> {
        counter: Mutex<usize>,
        executioner: B,
        runtime: Arc<tokio::runtime::Runtime>,
    }

    impl<B: Broadcaster<ID_SIZE>, const ID_SIZE: usize> AsyncTokioRuntime<B, ID_SIZE> {
        pub fn new(executioner: B, runtime: Arc<tokio::runtime::Runtime>) -> Self {
            Self {
                counter: Mutex::new(0),
                executioner,
                runtime,
            }
        }

        pub fn runtime(&self) -> &tokio::runtime::Runtime {
            &self.runtime
        }
    }

    impl<B: 'static + Broadcaster<ID_SIZE> + Send + Sync, const ID_SIZE: usize> Runtime
        for AsyncTokioRuntime<B, ID_SIZE>
    {
        fn register_timer(&self, duration: Duration) -> TimerId {
            let timer_id = {
                let mut lock = self.runtime.block_on(self.counter.lock());
                let id = *lock;
                *lock = id + 1;
                TimerId::from(id)
            };

            let broadcaster = self.executioner.clone();
            self.runtime.spawn(async move {
                tokio::time::sleep(duration).await;
                if let Err(e) = broadcaster.send_event(UseCaseEvent::Timer(timer_id)) {
                    log::error!("Failed to send Event to use cases: {}", e);
                }
            });

            timer_id
        }
    }
}
