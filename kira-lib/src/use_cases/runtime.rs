//! Interaction methods for [UseCase](crate::use_cases::UseCase) with (other) protocol instances.

use std::collections::binary_heap::PeekMut;
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::sync::mpsc::{self, SendError};
use std::time::{Duration, Instant};

use crate::domain::protocol_event::forwarding::ForwardingTablesUpdate;
use crate::use_cases::BroadcastableUseCaseEvent;
use crate::Output;
use crate::{
    messaging::ProtocolMessage,
    use_cases::{TimerId, UseCaseEvent},
};

#[derive(Debug, Clone, Eq, PartialEq)]
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

impl From<Timer> for UseCaseEvent {
    fn from(value: Timer) -> Self {
        Self::Timer(value.id)
    }
}

/// Interface for the [UseCases](crate::use_cases::UseCase) to the runtime environment.
#[derive(Debug, Clone)]
pub struct UseCaseRuntime {
    counter: usize,
    timers: BinaryHeap<Timer>,
    periodic_timers: HashMap<TimerId, Duration>,
    tx_events: VecDeque<UseCaseEvent>,
    output_channel: mpsc::SyncSender<Output>,
    current_time: Option<Instant>,
}

impl UseCaseRuntime {
    pub fn new(sender: mpsc::SyncSender<Output>) -> Self {
        Self {
            counter: 0,
            timers: BinaryHeap::with_capacity(14), // heuristic: 1 / UseCase
            periodic_timers: HashMap::default(),
            tx_events: VecDeque::with_capacity(5),
            output_channel: sender,
            current_time: None,
        }
    }
}

// R2Kad protocol instance orchestration methods
impl UseCaseRuntime {
    /// Feeds an event into the event pipeline and to all [UseCases](crate::use_cases::UseCase).
    pub(crate) fn spawn_event<E: Into<UseCaseEvent>>(&mut self, event: E) {
        self.tx_events.push_back(event.into());
    }

    /// Get next [UseCaseEvent] for processing by the use cases.
    ///
    /// Normally this will yield the next internally buffered use case
    /// but if a [Timer] is due it is going to yield a Timer event instead.
    pub(crate) fn next_event(&mut self, now: Instant) -> Option<UseCaseEvent> {
        self.current_time = Some(now);

        // TODO: figure out if it's advantageous to firstly buffer all due Timer events
        //  and then fire the first.

        // returning `Timer` events first
        // so all use cases are up-to-date before processing buffered events
        let due_timer = if let Some(next_timer) = self.timers.peek_mut() {
            if next_timer.due >= now {
                let due_timer = PeekMut::pop(next_timer);
                Some(due_timer)
            } else {
                None
            }
        } else {
            None
        };
        // &mut self.timers dropped
        if let Some(due_timer) = due_timer {
            // register periodic timer again with _same_ id
            if let Some(duration) = self.periodic_timers.get(&due_timer.id) {
                self.register_timer_with_id(duration.clone(), due_timer.id);
            }

            return Some(due_timer.into());
        }

        // yield buffered event
        self.tx_events.pop_front()
    }
}

// timers
impl UseCaseRuntime {
    fn register_timer_with_id(&mut self, duration: Duration, id: TimerId) {
        let due = self
            .current_time
            .expect("Registering should only happen in an event loop")
            + duration;
        let timer = Timer { due, id };

        self.timers.push(timer);
    }

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
    pub fn register_timer(&mut self, duration: Duration) -> TimerId {
        let id = self.counter.into();
        self.register_timer_with_id(duration, id);

        // TODO: handle overflow
        self.counter = self
            .counter
            .checked_add(1)
            .expect("TimerId overflow should not occure");

        id
    }

    pub fn register_periodic_timer(&mut self, duration: Duration) -> TimerId {
        let timer_id = self.register_timer(duration);

        let pre_existing = self.periodic_timers.insert(timer_id, duration);
        assert_eq!(pre_existing, None, "periodic timer should not pre exist");

        timer_id
    }

    /// Returns the next [Instant] a timer is due or None if there are no timers registered.
    pub(crate) fn next_timeout(&self) -> Option<&Instant> {
        self.timers.peek().map(|timer| &timer.due)
    }
}

// interaction capabilities with other use cases and other components
impl UseCaseRuntime {
    fn send_output(&self, output: Output) -> Result<(), SendError<Output>> {
        self.output_channel.send(output)
    }

    /// Sends a [ProtocolMessage] to a different peer.
    ///
    /// The message will be routed via the underlay neighbor
    /// corresponding to the specified `ulnid`.
    pub fn send_message<P: Into<ProtocolMessage>>(
        &self,
        protocol_message: P,
        //ulnid: UnderlayNeighborId,
    ) -> Result<(), SendError<Output>> {
        // FIXME: support sending to specified underlay neighbor conveniently
        let output = Output::SendProtocolMessage(protocol_message.into(), todo!("ulnid"));
        self.send_output(output)
    }

    /// Sends an [UpdateForwardingTables] request to the forwarding tables.
    pub fn update_fwd_tables(
        &self,
        update: ForwardingTablesUpdate,
    ) -> Result<(), SendError<Output>> {
        let output = Output::UpdateForwardingTables(update);
        self.send_output(output)
    }

    pub fn broadcast_event(&mut self, event: BroadcastableUseCaseEvent) {
        self.tx_events.push_back(event.into());
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::collections::HashSet;
    use std::time::Duration;

    #[test]
    fn no_timers_no_next_timeout() {
        let (sender, _) = mpsc::sync_channel(42);
        let runtime = UseCaseRuntime::new(sender);

        assert_eq!(runtime.next_timeout(), None);
    }

    #[test]
    fn ten_unique_timer_ids() {
        let (sender, _) = mpsc::sync_channel(42);
        let mut runtime = UseCaseRuntime::new(sender);

        let timers = 10;
        let mut seen_timers = HashSet::with_capacity(timers);

        for i in 0..timers {
            let timer_id = runtime.register_timer(Instant::now() + Duration::from_secs(i));
            assert!(!seen_timers.contains(&timer_id), "no duplicate timers");
            seen_timers.push(timer_id);
        }
    }

    // TODO: more unit tests
}
