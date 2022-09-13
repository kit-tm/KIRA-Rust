use std::fmt::{Display, Formatter};
use std::marker::PhantomData;
use std::time::Duration;

use rand::Rng;

use crate::context::UseCaseContext;
use crate::messaging::{HelloMessage, ProtocolMessageSender};
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{TimerId, UseCase, UseCaseEvent, UseCaseState};

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub struct PeriodicPNAdvertisingConfig {
    pub max_timeout: Duration,
    /// Initial timeout duration to use for sending PNHello messages.
    ///
    /// If this is bigger than `probing_timeout` this will only be used once on initialization
    /// and after that the `probing_timeout` will be used to calculate the timers duration.
    pub initial_timeout: Duration,
    /// Random scatter to not send PNHello messages in a fixed interval.
    pub max_scatter: Duration,
}

impl Default for PeriodicPNAdvertisingConfig {
    fn default() -> Self {
        Self {
            max_timeout: Duration::from_secs(30),
            initial_timeout: Duration::from_millis(250),
            max_scatter: Duration::from_millis(225),
        }
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum PeriodicPNAdvertisingState {
    Initialized,
    /// Contains the last timeout duration and the last timer id.
    Running(Duration, TimerId),
    Error,
}

impl UseCaseState for PeriodicPNAdvertisingState {
    fn is_error(&self) -> bool {
        self == &Self::Error
    }
}

#[derive(Debug)]
pub enum PeriodicPNAdvertisingError {
    SendError,
}

impl Display for PeriodicPNAdvertisingError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SendError => write!(f, "Sending a message failed"),
        }
    }
}

impl std::error::Error for PeriodicPNAdvertisingError {}

#[derive(Debug, Clone)]
pub struct PeriodicPNAdvertising<C, const BUCKET_SIZE: usize> {
    _pd: PhantomData<C>,
    state: PeriodicPNAdvertisingState,
    config: PeriodicPNAdvertisingConfig,
}

impl<C, const BUCKET_SIZE: usize> Default for PeriodicPNAdvertising<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self::new(PeriodicPNAdvertisingConfig::default())
    }
}

impl<C, const BUCKET_SIZE: usize> PeriodicPNAdvertising<C, BUCKET_SIZE> {
    pub fn new(config: PeriodicPNAdvertisingConfig) -> Self {
        Self {
            _pd: PhantomData::default(),
            state: PeriodicPNAdvertisingState::Initialized,
            config,
        }
    }

    /// Calculates the duration to wait before sending the next `PNHello`.
    ///
    /// Exponentially increases the last duration until the configured [PeriodicPNAdvertisingConfig::probing_timeout]
    /// is reached and then that duration will be used.
    ///
    /// Also introduces a randomized scatter to the duration given through
    /// [PeriodicPNAdvertisingConfig::max_scatter].
    fn next_timeout_duration(&self, last_duration: Duration) -> Duration {
        let next_timeout_increased = 2 * last_duration;
        let new_duration = if next_timeout_increased < self.config.max_timeout {
            next_timeout_increased
        } else {
            self.config.max_timeout
        };

        // Randomize in a given scatter interval
        let mut thread_rng = rand::thread_rng();
        let random_scatter = if self.config.max_scatter.is_zero() {
            Duration::ZERO
        } else {
            thread_rng.gen_range(Duration::from_millis(0)..self.config.max_scatter)
        };
        new_duration - self.config.max_scatter / 2 + random_scatter
    }
}

impl<C, const BUCKET_SIZE: usize> PeriodicPNAdvertising<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    C::MessageSender: ProtocolMessageSender,
{
    fn send_hello(&self, context: &C) -> Result<(), PeriodicPNAdvertisingError> {
        let message = HelloMessage {
            source: context.root_id().clone(),
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
        };
        log::trace!(target: "periodic_pn_advertising", "Sending message {:?}", message);
        if let Err(e) = context.message_sender_mut().send(message) {
            log::error!(target: "periodic_pn_advertising",
                "MessageSender failed: {}",
                e
            );
            return Err(PeriodicPNAdvertisingError::SendError);
        }

        Ok(())
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase for PeriodicPNAdvertising<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    C::MessageSender: ProtocolMessageSender,
{
    type Context = C;
    type Error = PeriodicPNAdvertisingError;
    type State = PeriodicPNAdvertisingState;

    fn start(&mut self, context: &C) -> Result<(), Self::Error> {
        let timer_id = context
            .runtime()
            .register_timer(self.config.initial_timeout);

        self.state = PeriodicPNAdvertisingState::Running(self.config.initial_timeout, timer_id);

        self.send_hello(context)
    }

    fn handle_event(&mut self, context: &C, event: UseCaseEvent) -> Result<(), Self::Error> {
        if let (
            UseCaseEvent::Timer(id),
            PeriodicPNAdvertisingState::Running(last_timeout, timer_id),
        ) = (event, self.state.clone())
        {
            if id == timer_id {
                self.send_hello(context)?;

                let next_timeout = self.next_timeout_duration(last_timeout);
                let timer_id = context.runtime().register_timer(next_timeout);
                self.state = PeriodicPNAdvertisingState::Running(next_timeout, timer_id);
            }
        }

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::Receiver;
    use std::time::Duration;

    use crate::broadcaster::MPSCBroadcaster;
    use crate::context::SyncContext;
    use crate::domain::physical_neighbor_table::PNTable;
    use crate::domain::{FlatRoutingTable, InsertionStrategyResult, NodeId, TestInsertionStrategy};
    use crate::messaging::tests::ArcSyncInMemoryMessageHub;
    use crate::runtime::ImmediateRuntime;
    use crate::use_cases::periodic_pn_advertising::{
        PeriodicPNAdvertising, PeriodicPNAdvertisingConfig, PeriodicPNAdvertisingState,
    };
    use crate::use_cases::{UseCase, UseCaseEvent};

    fn init_test_context() -> (
        NodeId,
        ArcSyncInMemoryMessageHub,
        MPSCBroadcaster,
        Receiver<UseCaseEvent>,
        SyncContext<
            FlatRoutingTable<20, 1>,
            ArcSyncInMemoryMessageHub,
            ImmediateRuntime<MPSCBroadcaster>,
            TestInsertionStrategy,
        >,
    ) {
        let root = NodeId::one();

        let routing_table = FlatRoutingTable::<20, 1>::new(root.clone())
            .expect("failed to build flat routing table");

        let hub = ArcSyncInMemoryMessageHub::new();

        let (broadcaster, broadcast_receiver) = MPSCBroadcaster::new(10);

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let context = SyncContext::new(
            root.clone(),
            routing_table,
            PNTable::new(),
            insertion_strategy,
            hub.clone(),
            ImmediateRuntime::new(broadcaster.clone()),
        );

        (root, hub, broadcaster, broadcast_receiver, context)
    }

    #[test]
    fn start_test() {
        let (_root_id, _hub, _broadcaster, broadcast_receiver, context) = init_test_context();

        let mut use_case = PeriodicPNAdvertising::<_, 20>::new(PeriodicPNAdvertisingConfig {
            max_timeout: Duration::from_secs(0),
            initial_timeout: Duration::from_secs(0),
            max_scatter: Duration::from_secs(0),
        });

        assert!(use_case.start(&context).is_ok());

        let timer_id = match &use_case.state {
            PeriodicPNAdvertisingState::Running(_, timer_id) => *timer_id,
            _ => panic!("Invalid state returned: {:?}", &use_case.state),
        };

        let event = broadcast_receiver
            .try_recv()
            .expect("failed to receive event");
        assert_eq!(event, UseCaseEvent::Timer(timer_id));
    }

    #[test]
    fn initial_timeout_used() {
        let (_root_id, _hub, _broadcaster, _broadcast_receiver, context) = init_test_context();

        let mut use_case = PeriodicPNAdvertising::<_, 20>::new(PeriodicPNAdvertisingConfig {
            max_timeout: Duration::from_secs(0),
            initial_timeout: Duration::from_secs(33),
            max_scatter: Duration::from_secs(0),
        });

        assert!(use_case.start(&context).is_ok());

        let (timer_duration, _timer_id) = match &use_case.state {
            PeriodicPNAdvertisingState::Running(timer_duration, timer_id) => {
                (*timer_duration, *timer_id)
            }
            _ => panic!("Invalid state returned: {:?}", &use_case.state),
        };

        assert_eq!(
            timer_duration,
            Duration::from_secs(33),
            "Timeout should be configured initial timeout 33s but was {:?}",
            timer_duration
        );
    }

    #[test]
    fn exponentially_increased_until_max_timeout() {
        let (_root_id, _hub, _broadcaster, _broadcast_receiver, context) = init_test_context();

        let mut use_case = PeriodicPNAdvertising::<_, 20>::new(PeriodicPNAdvertisingConfig {
            max_timeout: Duration::from_millis(30),
            initial_timeout: Duration::from_millis(8),
            max_scatter: Duration::from_millis(0),
        });

        assert!(use_case.start(&context).is_ok());

        let (timer_duration, timer_id) = match &use_case.state {
            PeriodicPNAdvertisingState::Running(timer_duration, timer_id) => {
                (*timer_duration, *timer_id)
            }
            _ => panic!("Invalid state returned: {:?}", &use_case.state),
        };
        assert_eq!(
            timer_duration,
            Duration::from_millis(8),
            "Timeout should be configured initial timeout 8ms but was {:?}",
            timer_duration
        );

        let handle_result = use_case.handle_event(&context, UseCaseEvent::Timer(timer_id));
        assert!(
            handle_result.is_ok(),
            "Handling timer event returned error: {:?}",
            handle_result
        );
        let (next_timer_duration, timer_id) = match &use_case.state {
            PeriodicPNAdvertisingState::Running(timer_duration, timer_id) => {
                (*timer_duration, *timer_id)
            }
            _ => panic!("Invalid state returned: {:?}", &use_case.state),
        };
        assert_eq!(
            next_timer_duration,
            Duration::from_millis(16),
            "Timeout should be exponentially increased to 16ms but was: {:?}",
            next_timer_duration
        );

        let handle_result = use_case.handle_event(&context, UseCaseEvent::Timer(timer_id));
        assert!(
            handle_result.is_ok(),
            "Handling timer event returned error: {:?}",
            handle_result
        );
        let (next_timer_duration, _timer_id) = match &use_case.state {
            PeriodicPNAdvertisingState::Running(timer_duration, timer_id) => {
                (*timer_duration, *timer_id)
            }
            _ => panic!("Invalid state returned: {:?}", &use_case.state),
        };
        assert_eq!(
            next_timer_duration,
            Duration::from_millis(30),
            "Timeout should be exponentially increased until max_timeout (30ms) but was: {:?}",
            next_timer_duration
        );
    }
}
