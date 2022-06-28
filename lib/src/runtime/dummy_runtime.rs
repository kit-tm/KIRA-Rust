use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::broadcaster::Broadcaster;
use crate::runtime::Runtime;
use crate::use_cases::{TimerId, UseCaseEvent};

/// Runtime emitting timers immediately.
///
/// Doesn't handle id overflow.
#[derive(Debug, Clone)]
pub struct DummyRuntime<B> {
    broadcaster: Arc<B>,
    counter: Arc<AtomicUsize>,
}

impl<B> DummyRuntime<B> {
    pub fn new(broadcaster: Arc<B>) -> Self {
        Self {
            broadcaster,
            counter: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn counter(&self) -> usize {
        self.counter.load(Ordering::Relaxed)
    }
}

impl<B: Broadcaster> Runtime for DummyRuntime<B>
where
    B: 'static + Broadcaster + Send + Sync,
{
    /// Instantly emits the event returning its id.
    fn register_timer(&self, _duration: Duration) -> TimerId {
        let id = TimerId::from(self.counter.fetch_add(1, Ordering::Relaxed));
        let _ = self.broadcaster.send_event(UseCaseEvent::Timer(id));
        id
    }

    /// Emits endless events immediately as soon as the broadcaster has space.
    fn register_periodic_timer(&self, _duration: Duration) -> TimerId {
        let id = TimerId::from(self.counter.fetch_add(1, Ordering::Relaxed));
        let broadcaster = Arc::clone(&self.broadcaster);
        let _ = broadcaster.send_event(UseCaseEvent::Timer(id));
        std::thread::spawn(move || loop {
            if broadcaster.send_event(UseCaseEvent::Timer(id)).is_err() {
                break;
            }
        });
        id
    }
}
