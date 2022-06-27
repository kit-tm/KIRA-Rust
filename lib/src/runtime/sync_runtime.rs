use std::collections::HashMap;
use std::ops::Deref;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::broadcaster::Broadcaster;
use crate::runtime::Runtime;
use crate::use_cases::{TimerId, UseCaseEvent};

/// Single Threaded Runtime using the standard library.
///
/// # Attention
///
/// The current implementation spawns a thread for every timer.
///
/// **TODO**: Optimize.
pub struct SyncRuntime<E> {
    id_counter: AtomicUsize,
    broadcaster: Arc<E>,
    timers: Arc<Mutex<HashMap<TimerId, JoinHandle<()>>>>,
}

impl<E: Default> Default for SyncRuntime<E> {
    fn default() -> Self {
        Self {
            id_counter: AtomicUsize::new(0),
            broadcaster: Arc::new(E::default()),
            timers: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl<E> SyncRuntime<E> {
    pub fn new(executioner: Arc<E>) -> Self {
        SyncRuntime {
            id_counter: AtomicUsize::new(0),
            broadcaster: executioner,
            timers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn add_timer(&self, timer_id: TimerId, handle: JoinHandle<()>) {
        let mut lock = self.timers.lock().expect("failed to get timers lock");
        lock.insert(timer_id, handle);
    }

    fn remove_timer(timers: Arc<Mutex<HashMap<TimerId, JoinHandle<()>>>>, timer_id: &TimerId) {
        let mut lock = timers.lock().expect("failed to get timers lock");
        lock.remove(timer_id);
    }
}

impl<E: Broadcaster> SyncRuntime<E> {
    fn wait_and_send_event(
        broadcaster: &E,
        timer_id: TimerId,
        duration: Duration,
    ) -> Result<(), ()> {
        std::thread::sleep(duration);
        if let Err(e) = broadcaster.send_event(UseCaseEvent::Timer(timer_id)) {
            log::error!("Failed to send Event to use cases: {}", e);
            return Err(());
        }

        Ok(())
    }
}

impl<E: 'static + Broadcaster + Send + Sync> Runtime for SyncRuntime<E> {
    fn register_timer(&self, duration: Duration) -> TimerId {
        let timer_id = TimerId::from(self.id_counter.fetch_add(1, Ordering::Relaxed));

        let broadcaster = Arc::clone(&self.broadcaster);
        let timers = Arc::clone(&self.timers);
        let handle = std::thread::spawn(move || {
            // One-Shot Errors don't need to be handled
            let _ = SyncRuntime::wait_and_send_event(broadcaster.deref(), timer_id, duration);
            SyncRuntime::<E>::remove_timer(timers, &timer_id);
        });
        self.add_timer(timer_id, handle);

        timer_id
    }

    fn register_periodic_timer(&self, duration: Duration) -> TimerId {
        let timer_id = TimerId::from(self.id_counter.fetch_add(1, Ordering::Relaxed));

        let broadcaster = Arc::clone(&self.broadcaster);
        let timers = Arc::clone(&self.timers);
        let handle = std::thread::spawn(move || {
            loop {
                if SyncRuntime::wait_and_send_event(broadcaster.deref(), timer_id, duration)
                    .is_err()
                {
                    break;
                }
            }
            SyncRuntime::<E>::remove_timer(timers, &timer_id);
        });
        self.add_timer(timer_id, handle);

        timer_id
    }
}
