use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::broadcaster::Broadcaster;
use crate::domain::NetworkInterface;
use crate::messaging::ProtocolMessage;
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{TimerId, UseCaseEvent};

/// Runtime emitting timers immediately.
///
/// Doesn't handle id overflow.
///
/// Doesn't remove old timers from its timer-duration tracking.
#[derive(Debug, Clone)]
pub struct ImmediateRuntime<B: Clone + Send + Sync> {
    broadcaster: B,
    counter: Arc<AtomicUsize>,
    timers: Arc<Mutex<HashMap<TimerId, Duration>>>,
}

impl<B: Clone + Send + Sync> ImmediateRuntime<B> {
    /// Create a new runtime with a given broadcaster.
    pub fn new(broadcaster: B) -> Self {
        Self {
            broadcaster,
            counter: Arc::new(AtomicUsize::new(0)),
            timers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Returns the [TimerId] of the latest timer created if any.
    pub fn latest_timer_id(&self) -> Option<TimerId> {
        let value = self.counter.load(Ordering::Relaxed);
        if value == 0 {
            None
        } else {
            Some(TimerId::from(value - 1))
        }
    }

    /// Returns the duration of a timer with the given [TimerId] if present.
    pub fn get_timer_duration(&self, timer_id: &TimerId) -> Option<Duration> {
        let lock = self.timers.lock().expect("failed to get lock on timers");
        let value = lock.get(timer_id);
        value.cloned()
    }
}

impl<B: Broadcaster> UseCaseRuntime for ImmediateRuntime<B>
where
    B: 'static + Broadcaster + Send + Sync,
{
    type SendError = B::SendError;

    /// Instantly emits the event returning its id.
    fn register_timer(&self, duration: Duration) -> TimerId {
        let id = TimerId::from(self.counter.fetch_add(1, Ordering::Relaxed));
        self.timers
            .lock()
            .expect("failed to get lock on timers")
            .insert(id, duration);
        let _ = self.broadcaster.send_event(UseCaseEvent::Timer(id));
        id
    }

    /// Emits endless events immediately as soon as the broadcaster has space.
    fn register_periodic_timer(&self, duration: Duration) -> TimerId {
        let id = TimerId::from(self.counter.fetch_add(1, Ordering::Relaxed));
        self.timers
            .lock()
            .expect("failed to get lock on timers")
            .insert(id, duration);
        let broadcaster = self.broadcaster.clone();
        let _ = broadcaster.send_event(UseCaseEvent::Timer(id));
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            if broadcaster.send_event(UseCaseEvent::Timer(id)).is_err() {
                break;
            }
        });
        id
    }

    fn send_message(&self, message: ProtocolMessage) -> Result<(), Self::SendError> {
        let use_case_broadcaster = self.broadcaster.clone();
        let event = UseCaseEvent::Message(message, NetworkInterface::loopback());

        if let Err(e) = use_case_broadcaster.send_event(event) {
            log::error!("Failed to send Event to use cases: {:?}", e);
            return Err(e)
        }

        Ok(())
    }
}
