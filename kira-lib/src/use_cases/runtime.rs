//! Interaction methods for [UseCase](crate::use_cases::UseCase) with (other) protocol instances.

use std::collections::{BinaryHeap, VecDeque};
use std::sync::mpsc::{self, SendError};
use std::time::{Duration, Instant};

use crate::domain::protocol_event::forwarding::ForwardingTablesUpdate;
use crate::utils::sync::Sender;
use crate::Output;
use crate::{
    domain::underlay::UnderlayNeighborId,
    messaging::ProtocolMessage,
    use_cases::{TimerId, UseCaseEvent},
};

#[derive(Clone, Eq, PartialEq)]
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

/// Interface for the [UseCases](crate::use_cases::UseCase) to the runtime environment.
pub struct UseCaseRuntime {
    counter: usize,
    timers: BinaryHeap<Timer>,
    tx_events: VecDeque<UseCaseEvent>,
    output_channel: mpsc::SyncSender<Output>,
    current_time: Option<Instant>,
}

impl UseCaseRuntime {
    pub fn new(sender: mpsc::SyncSender<Output>) -> Self {
        Self {
            counter: 0,
            timers: BinaryHeap::with_capacity(14), // heuristic: 1 / UseCase
            tx_events: VecDeque::with_capacity(5),
            output_channel: sender,
            current_time: None,
        }
    }
}

impl UseCaseRuntime {
    /// Feeds an event into the event pipeline and to all [UseCases](crate::use_cases::UseCase).
    pub(crate) fn spawn_event(&self, event: UseCaseEvent) {
        self.tx_events.push_back(event);
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
        if let Some(mut next_timer) = self.timers.peek_mut() {
            if *next_timer.due >= now {
                let Timer { due, id } = next_timer.pop();
                return Some(UseCaseEvent::Timer(id));
            }
        }

        // yield buffered event
        self.tx_events.pop_front()
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
    /// If called outside of an event loop.
    pub fn register_timer(&mut self, duration: Duration) -> TimerId {
        let due = self
            .current_time
            .expect("Registering should only happen in an event loop");
        let id = self.counter.into();
        let timer = Timer { due, id };

        self.timers.push(timer);
        // TODO: handle overflow
        self.counter = self
            .counter
            .checked_add(1)
            .expect("TimerId overflow should not occure");

        Ok(id)
    }

    /// Returns the next [Instant] a timer is due or None if there are no timers registered.
    pub(crate) fn next_timeout(&self) -> Option<&Instant> {
        self.timers.peek()
    }
}

impl UseCaseRuntime {
    fn send_output(&self, output: Output) -> Result<(), SendError<Output>> {
        self.output_channel.send(output)
    }

    /// Sends a [ProtocolMessage] to a different peer.
    ///
    /// The message will be routed via the underlay neighbor
    /// corresponding to the specified `ulnid`.
    pub fn send_message(
        &self,
        protocol_message: ProtocolMessage,
        ulnid: UnderlayNeighborId,
    ) -> Result<(), SendError<Output>> {
        let output = Output::SendProtocolMessage(protocol_message, ulnid);
        self.send_output(output)
    }

    /// Sends an [UpdateForwardingTables] request to the forwarding tables.
    pub fn update_fwd_tables(
        &self,
        update: ForwardingTablesUpdate,
    ) -> Result<(), SendError<Output>> {
        let output = Output::UpdateForwardingTables(());
        self.send_output(output)
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
