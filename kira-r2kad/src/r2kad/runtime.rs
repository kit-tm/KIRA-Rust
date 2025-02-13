use std::collections::binary_heap::PeekMut;
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};

use crate::domain::protocol_event::forwarding::ForwardingTablesUpdate;
use crate::domain::UnderlayNeighborDestination;
use crate::runtime::UseCaseRuntime;
use crate::use_cases::BroadcastableUseCaseEvent;
use crate::Output;
use crate::{
    messaging::ProtocolMessage,
    use_cases::{TimerId, UseCaseEvent},
};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct Timer {
    due: Instant,
    id: TimerId,
}

// reverse the order to get a min heap
impl Ord for Timer {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.due
            .cmp(&other.due)
            .reverse()
            .then_with(|| self.id.cmp(&other.id))
    }
}

impl PartialOrd for Timer {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// [UseCaseRuntime] implementation for the [R2Kad][super::R2Kad] protocol instance.
#[derive(Debug)]
pub struct R2KadRuntime {
    counter: Mutex<usize>,
    timers: RwLock<BinaryHeap<Timer>>,
    periodic_timers: RwLock<HashMap<TimerId, Duration>>,
    // Next events to be processed *excluding* timer events.
    // We can't just save them in `R2Kad` since `UseCases` are allowed
    // to broadcast certain events via the runtime.
    event_queue: RwLock<VecDeque<UseCaseEvent>>,
    output_queue: RwLock<VecDeque<Output>>,
    current_time: Mutex<Instant>,
}

impl R2KadRuntime {
    /// Create new [UseCaseRuntime] with a specified startup time.
    pub fn with_startup_time(startup_time: Instant) -> Self {
        Self {
            counter: Mutex::default(),
            timers: RwLock::new(BinaryHeap::with_capacity(14)), // heuristic: 1 / UseCase
            periodic_timers: RwLock::default(),
            event_queue: RwLock::default(),
            output_queue: RwLock::default(),
            current_time: Mutex::new(startup_time),
        }
    }

    pub fn new() -> Self {
        Self::with_startup_time(Instant::now())
    }
}

impl Default for R2KadRuntime {
    fn default() -> Self {
        Self::new()
    }
}

// R2Kad protocol instance orchestration methods
impl R2KadRuntime {
    pub fn set_current_time(&self, now: Instant) {
        *self.current_time.lock().unwrap() = now;
    }

    /// Returns next due timer based on current time of the runtime.
    pub fn next_timer(&self) -> Option<TimerId> {
        let due_timer = if let Some(next_timer) = self.timers.write().unwrap().peek_mut() {
            if next_timer.due <= *self.current_time.lock().unwrap() {
                PeekMut::pop(next_timer)
            } else {
                // impossible to yield more timers because of monoton increasing heap
                return None;
            }
        } else {
            // no timers present => no need to check again
            log::warn!("no runtime timers set");
            return None;
        };

        // register periodic timer under same id to fire again
        if let Some(duration) = self.periodic_timers.read().unwrap().get(&due_timer.id) {
            log::trace!(
                "register periodic timer again: {:?} ({:?})",
                due_timer.id,
                duration
            );
            self.register_timer_with_id(*duration, due_timer.id);
        }

        Some(due_timer.id)
    }

    pub fn next_event(&self) -> Option<UseCaseEvent> {
        self.event_queue.write().unwrap().pop_front()
    }

    /// Retrieve next [Output] event.
    ///
    /// This function should be called immediately after no [UseCaseEvents](UseCaseEvent)
    /// are left to process by use cases yielded by [next_event](Self::next_event).
    pub fn poll_output(&self) -> Option<Output> {
        self.output_queue.write().unwrap().pop_front()
    }

    /// Current time of the runtime.
    pub fn current_time(&self) -> Instant {
        *self.current_time.lock().unwrap()
    }

    /// Returns the next [Instant] a timer is due.
    ///
    /// If there are no timers registered [None](Option::None) is returned.
    pub fn poll_timeout(&self) -> Option<Instant> {
        self.timers.read().unwrap().peek().map(|timer| timer.due)
    }
}

// Helper methods
impl R2KadRuntime {
    fn register_timer_with_id(&self, duration: Duration, id: TimerId) {
        // this is so we can optimize [Self::next_event]
        debug_assert!(!duration.is_zero(), "timers should have positive durations");

        let due = *self.current_time.lock().unwrap() + duration;
        let timer = Timer { due, id };

        self.timers.write().unwrap().push(timer);
        log::trace!("registered timer: {:?} ({:?})", timer, duration);
    }

    fn send_output(&self, output: Output) {
        self.output_queue.write().unwrap().push_back(output)
    }
}

impl UseCaseRuntime for R2KadRuntime {
    /// Creates a timer which will later yield a TimerEvent.
    ///
    /// The returned TimerId is unique.
    ///
    ///
    /// # Examples
    ///
    /// ```
    /// let runtime = UseCaseRuntime::default();
    /// assert_eq!(runtime.next_event(Instance::now()), None);
    ///
    /// let timer_id = runtime.register_timer(Instant::now() + Duration::from_secs(5));
    /// std::thread::sleep(5);
    /// assert_eq!(runtime.next_event(Instance::now()), UseCaseEvent::Timer(timer_id));
    /// ```
    ///
    /// # Panics
    ///
    /// If called outside of an event loop or on overflow.
    fn register_timer(&self, duration: Duration) -> TimerId {
        assert!(
            !duration.is_zero(),
            "Timers should have a positive duration"
        );

        let mut counter = self.counter.lock().unwrap();
        let id = (*counter).into();
        self.register_timer_with_id(duration, id);

        // TODO: handle overflow
        *counter = counter
            .checked_add(1)
            .expect("TimerId overflow should not occure");

        id
    }

    fn register_periodic_timer(&self, duration: Duration) -> TimerId {
        let timer_id = self.register_timer(duration);

        let pre_existing = self
            .periodic_timers
            .write()
            .unwrap()
            .insert(timer_id, duration);
        assert_eq!(pre_existing, None, "periodic timer should not pre exist");

        timer_id
    }

    /// Sends a [ProtocolMessage] to a different peer.
    ///
    /// The message will be routed via the underlay neighbor
    /// corresponding to the specified `ulnid`.
    fn send_message_via<P: Into<ProtocolMessage>>(
        &self,
        protocol_message: P,
        ulnid: UnderlayNeighborDestination,
    ) {
        let output = Output::SendProtocolMessage(protocol_message.into(), ulnid);
        self.send_output(output)
    }

    /// Sends an [ForwardingTablesUpdate] request to the forwarding tables.
    fn update_fwd_tables<U: Into<ForwardingTablesUpdate>>(&self, update: U) {
        let update = update.into();
        let output = Output::UpdateForwardingTables(update);
        self.send_output(output)
    }

    /// Broadcast an [UseCaseEvent](BroadcastableUseCaseEvent).
    fn broadcast_event<B: Into<BroadcastableUseCaseEvent>>(&self, event: B) {
        self.event_queue
            .write()
            .unwrap()
            .push_back(event.into().into());
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::collections::HashSet;
    use std::time::Duration;

    #[test]
    fn runtime_is_empty_on_new() {
        let runtime = R2KadRuntime::new();

        assert_eq!(runtime.poll_timeout(), None, "No initial timers set");
    }

    #[test]
    fn ten_unique_timer_ids() {
        let runtime = R2KadRuntime::new();

        let timers = 10;
        let mut seen_timers = HashSet::with_capacity(timers);

        for i in 1..timers {
            let timer_id = runtime.register_timer(Duration::from_secs(i as u64));
            assert!(!seen_timers.contains(&timer_id), "No duplicate timers");
            seen_timers.insert(timer_id);
        }
    }

    #[test]
    fn no_time_elapse() {
        let runtime = R2KadRuntime::new();
        let initial_time = runtime.current_time();

        let _ = runtime.register_timer(Duration::from_secs(42));
        let current_time = runtime.current_time();

        assert_eq!(
            initial_time, current_time,
            "No time change on registering a timer"
        );
    }
    #[test]
    fn next_timer() {
        let runtime = R2KadRuntime::new();
        let now = Instant::now();
        runtime.set_current_time(now);

        let duration = Duration::from_secs(42);
        let id = runtime.register_timer(duration);

        assert!(
            runtime.next_timer().is_none(),
            "timer should not fire immediately"
        );
        runtime.set_current_time(now + duration / 2);
        assert!(
            runtime.next_timer().is_none(),
            "timer should not fire if not due"
        );
        runtime.set_current_time(now + duration);
        assert_eq!(runtime.next_timer(), Some(id), "timer should fire on due");
    }

    // TODO: more unit tests
}
