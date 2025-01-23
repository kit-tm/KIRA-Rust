use std::cell::{Cell, RefCell};
use std::collections::binary_heap::PeekMut;
use std::collections::{BinaryHeap, HashMap, VecDeque};
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
#[derive(Debug, Clone)]
pub struct R2KadRuntime {
    counter: Cell<usize>,
    timers: RefCell<BinaryHeap<Timer>>,
    periodic_timers: RefCell<HashMap<TimerId, Duration>>,
    // Next events to be processed *excluding* timer events.
    // We can't just save them in `R2Kad` since `UseCases` are allowed
    // to broadcast certain events via the runtime.
    event_queue: RefCell<VecDeque<UseCaseEvent>>,
    output_queue: RefCell<VecDeque<Output>>,
    current_time: Cell<Instant>,
}

impl R2KadRuntime {
    /// Create new [UseCaseRuntime] with a specified startup time.
    pub fn with_startup_time(startup_time: Instant) -> Self {
        Self {
            counter: Cell::default(),
            timers: RefCell::new(BinaryHeap::with_capacity(14)), // heuristic: 1 / UseCase
            periodic_timers: RefCell::default(),
            event_queue: RefCell::default(),
            output_queue: RefCell::default(),
            current_time: Cell::new(startup_time),
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
        let _ = self.current_time.replace(now);
    }

    /// Returns next due timer based on current time of the runtime.
    pub fn next_timer(&self) -> Option<TimerId> {
        let due_timer = if let Some(next_timer) = self.timers.borrow_mut().peek_mut() {
            if next_timer.due >= self.current_time.get() {
                PeekMut::pop(next_timer)
            } else {
                // impossible to yield more timers because of monoton increasing heap
                return None;
            }
        } else {
            // no timers present => no need to check again
            return None;
        };

        // register periodic timer under same id to fire again
        if let Some(duration) = self.periodic_timers.borrow().get(&due_timer.id) {
            self.register_timer_with_id(*duration, due_timer.id);
        }

        Some(due_timer.id)
    }

    pub fn next_event(&self) -> Option<UseCaseEvent> {
        self.event_queue.borrow_mut().pop_front()
    }

    /// Retrieve next [Output] event.
    ///
    /// This function should be called immediately after no [UseCaseEvents](UseCaseEvent)
    /// are left to process by use cases yielded by [next_events](Self::next_events).
    pub fn poll_output(&self) -> Option<Output> {
        self.output_queue.borrow_mut().pop_front()
    }

    /// Current time of the runtime.
    pub fn current_time(&self) -> Instant {
        self.current_time.get()
    }

    /// Returns the next [Instant] a timer is due.
    ///
    /// If there are no timers registered [None](Option::None) is returned.
    pub fn poll_timeout(&self) -> Option<Instant> {
        self.timers.borrow().peek().map(|timer| timer.due)
    }
}

// Helper methods
impl R2KadRuntime {
    fn register_timer_with_id(&self, duration: Duration, id: TimerId) {
        // this is so we can optimize [Self::next_events]
        assert!(!duration.is_zero(), "timers should have positive durations");

        let due = self.current_time.get() + duration;
        let timer = Timer { due, id };

        self.timers.borrow_mut().push(timer);
    }

    fn send_output(&self, output: Output) {
        self.output_queue.borrow_mut().push_back(output)
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

        let counter = self.counter.get();
        let id = counter.into();
        self.register_timer_with_id(duration, id);

        // TODO: handle overflow
        self.counter.replace(
            counter
                .checked_add(1)
                .expect("TimerId overflow should not occure"),
        );

        id
    }

    fn register_periodic_timer(&self, duration: Duration) -> TimerId {
        let timer_id = self.register_timer(duration);

        let pre_existing = self.periodic_timers.borrow_mut().insert(timer_id, duration);
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
    fn broadcast_event(&self, event: BroadcastableUseCaseEvent) {
        self.event_queue.borrow_mut().push_back(event.into());
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

        for i in 0..timers {
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

    // TODO: more unit tests
}
