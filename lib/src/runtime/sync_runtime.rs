use crate::broadcaster::Broadcaster;
use crate::runtime::Runtime;
use crate::use_cases::{TimerId, UseCaseEvent};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Single Threaded Runtime using the standard library.
pub struct SyncRuntime<E> {
    id_counter: AtomicUsize,
    broadcaster: E,
}

impl<E: Default> Default for SyncRuntime<E> {
    fn default() -> Self {
        Self {
            id_counter: AtomicUsize::new(0),
            broadcaster: E::default(),
        }
    }
}

impl<E> SyncRuntime<E> {
    pub const fn new(executioner: E) -> Self {
        SyncRuntime {
            id_counter: AtomicUsize::new(0),
            broadcaster: executioner,
        }
    }
}

impl<E: Broadcaster> Runtime for SyncRuntime<E> {
    fn register_timer(&self, duration: Duration) -> TimerId {
        std::thread::sleep(duration);
        let timer_id = TimerId::from(self.id_counter.fetch_add(1, Ordering::Relaxed));
        if let Err(e) = self.broadcaster.send_event(UseCaseEvent::Timer(timer_id)) {
            log::error!("Failed to send Event to use cases: {}", e);
        }
        timer_id
    }
}
