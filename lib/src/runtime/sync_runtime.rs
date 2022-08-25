use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::broadcaster::Broadcaster;
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{TimerId, UseCaseEvent};

/// Single Threaded Runtime using the standard library.
///
/// # Attention
///
/// The current implementation spawns a thread for every timer.
///
/// **TODO**: Optimize.
pub struct SyncRuntime<B: Sync + Send> {
    id_counter: AtomicUsize,
    broadcaster: B,
    timers: Arc<Mutex<HashMap<TimerId, JoinHandle<()>>>>,
}

impl<B: Default + Sync + Send> Default for SyncRuntime<B> {
    fn default() -> Self {
        Self {
            id_counter: AtomicUsize::new(0),
            broadcaster: B::default(),
            timers: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl<B: Sync + Send> SyncRuntime<B> {
    pub fn new(broadcaster: B) -> Self {
        SyncRuntime {
            id_counter: AtomicUsize::new(0),
            broadcaster,
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

impl<B> SyncRuntime<B>
where
    B: Broadcaster + Sync + Send,
    B::SendError: Debug,
{
    fn wait_and_send_event(
        broadcaster: &B,
        timer_id: TimerId,
        duration: Duration,
    ) -> Result<(), ()> {
        std::thread::sleep(duration);
        if let Err(e) = broadcaster.send_event(UseCaseEvent::Timer(timer_id)) {
            log::error!("Failed to send Event to use cases: {:?}", e);
            return Err(());
        }

        Ok(())
    }
}

impl<B> UseCaseRuntime for SyncRuntime<B>
where
    B: 'static + Broadcaster + Send + Sync,
    B::SendError: Debug,
{
    fn register_timer(&self, duration: Duration) -> TimerId {
        let timer_id = TimerId::from(self.id_counter.fetch_add(1, Ordering::Relaxed));

        let broadcaster = self.broadcaster.clone();
        let timers = Arc::clone(&self.timers);
        let handle = std::thread::spawn(move || {
            // One-Shot Errors don't need to be handled
            let _ = SyncRuntime::wait_and_send_event(&broadcaster, timer_id, duration);
            SyncRuntime::<B>::remove_timer(timers, &timer_id);
        });
        self.add_timer(timer_id, handle);

        timer_id
    }

    fn register_periodic_timer(&self, duration: Duration) -> TimerId {
        let timer_id = TimerId::from(self.id_counter.fetch_add(1, Ordering::Relaxed));

        let broadcaster = self.broadcaster.clone();
        let timers = Arc::clone(&self.timers);
        let handle = std::thread::spawn(move || {
            loop {
                if SyncRuntime::wait_and_send_event(&broadcaster, timer_id, duration).is_err() {
                    break;
                }
            }
            SyncRuntime::<B>::remove_timer(timers, &timer_id);
        });
        self.add_timer(timer_id, handle);

        timer_id
    }
}

#[cfg(all(test, feature = "bus"))]
mod tests {
    use std::time::{Duration, Instant};

    use crate::broadcaster::Broadcaster;
    use crate::runtime::{SyncRuntime, UseCaseRuntime};
    use crate::use_cases::UseCaseEvent;

    #[test]
    fn register_timer() {
        let broadcaster = crate::broadcaster::BusBroadcaster::new(1);
        let mut broadcast_receiver = broadcaster.subscribe();

        let runtime = SyncRuntime::new(broadcaster);

        let start = Instant::now();
        let id = runtime.register_timer(Duration::from_millis(10));

        let event = broadcast_receiver.recv();
        let end = start.elapsed();

        assert!(event.is_ok(), "{:?}", event);
        let event = event.unwrap();

        assert_eq!(event, UseCaseEvent::Timer(id));
        assert!(end >= Duration::from_millis(10));
    }

    #[test]
    fn register_periodic_timer() {
        let broadcaster = crate::broadcaster::BusBroadcaster::new(1);
        let mut broadcast_receiver = broadcaster.subscribe();

        let runtime = SyncRuntime::new(broadcaster);

        let start = Instant::now();
        let id = runtime.register_periodic_timer(Duration::from_millis(10));

        let event = broadcast_receiver.recv();
        let end = start.elapsed();

        assert!(event.is_ok(), "{:?}", event);
        let event = event.unwrap();

        assert_eq!(event, UseCaseEvent::Timer(id));
        assert!(end >= Duration::from_millis(10));

        let event = broadcast_receiver.recv();
        let end = start.elapsed();

        assert!(event.is_ok(), "{:?}", event);
        let event = event.unwrap();

        assert_eq!(event, UseCaseEvent::Timer(id));
        assert!(end >= Duration::from_millis(20));
    }
}
