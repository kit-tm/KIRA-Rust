use std::fmt::{Display, Formatter};
use std::marker::PhantomData;
use std::time::Duration;

use crate::context::UseCaseContext;
use crate::domain::NodeId;
use crate::messaging::{HelloMessage, ProtocolMessageSender};
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{TimerId, UseCase, UseCaseEvent, UseCaseState};

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub struct Config {
    pub probing_timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            // TODO: Useful timeout duration?
            probing_timeout: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum State {
    Initialized,
    Running(TimerId),
    Error,
}

impl UseCaseState for State {
    fn is_finished(&self) -> bool {
        false
    }

    fn is_error(&self) -> bool {
        self == &Self::Error
    }
}

#[derive(Debug)]
pub enum Error {
    SendError,
}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SendError => write!(f, "Sending a message failed"),
        }
    }
}

impl std::error::Error for Error {}

#[derive(Debug, Clone)]
pub struct PeriodicPNAdvertising<C, const BUCKET_SIZE: usize> {
    _pd: PhantomData<C>,
    state: State,
    config: Config,
}

impl<C, const BUCKET_SIZE: usize> Default for PeriodicPNAdvertising<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self::new(Config::default())
    }
}

impl<C, const BUCKET_SIZE: usize> PeriodicPNAdvertising<C, BUCKET_SIZE> {
    pub fn new(config: Config) -> Self {
        Self {
            _pd: PhantomData::default(),
            state: State::Initialized,
            config,
        }
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase for PeriodicPNAdvertising<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    C::MessageSender: ProtocolMessageSender,
{
    type Context = C;
    type Error = Error;
    type State = State;

    fn start(&mut self, context: &C) -> Result<(), Self::Error> {
        let timer_id = context
            .runtime()
            .register_periodic_timer(self.config.probing_timeout);

        self.state = State::Running(timer_id);

        Ok(())
    }

    fn handle_event(&mut self, context: &C, event: UseCaseEvent) -> Result<(), Self::Error> {
        if let (UseCaseEvent::Timer(id), State::Running(timer_id)) = (event, self.state.clone()) {
            if id == timer_id {
                if let Err(e) = context.message_sender_mut().send(HelloMessage {
                    source: context.root_id().clone(),
                    source_state_seq_nr: *context.neighbor_table().state_seq_nr(),
                    destination: NodeId::zero(),
                }) {
                    log::error!("MessageSender failed: {}", e);
                    self.state = State::Error;
                    return Err(Error::SendError);
                }
            }
        }

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

#[cfg(all(test, feature = "bus"))]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use crate::broadcaster::{Broadcaster, BusBroadcaster};
    use crate::context::SyncContext;
    use crate::domain::neighbor_table::NeighborTable;
    use crate::domain::{FlatRoutingTable, InsertionStrategyResult, NodeId, TestInsertionStrategy};
    use crate::messaging::tests::ArcSyncInMemoryMessageHub;
    use crate::runtime::DummyRuntime;
    use crate::use_cases::periodic_pn_advertising::{Config, PeriodicPNAdvertising, State};
    use crate::use_cases::{UseCase, UseCaseEvent};

    fn init_test_context() -> (
        NodeId,
        ArcSyncInMemoryMessageHub,
        Arc<BusBroadcaster>,
        SyncContext<
            FlatRoutingTable<20, 1>,
            ArcSyncInMemoryMessageHub,
            DummyRuntime<BusBroadcaster>,
            TestInsertionStrategy,
        >,
    ) {
        let root = NodeId::one();

        let routing_table = FlatRoutingTable::<20, 1>::new(root.clone())
            .expect("failed to build flat routing table");

        let hub = ArcSyncInMemoryMessageHub::new();

        let broadcaster = Arc::new(BusBroadcaster::new(1));

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let context = SyncContext::new(
            root.clone(),
            routing_table,
            NeighborTable::new(),
            insertion_strategy,
            hub.clone(),
            DummyRuntime::new(Arc::clone(&broadcaster)),
        );

        (root, hub, broadcaster, context)
    }

    #[test]
    fn start_test() {
        let (_root_id, _hub, broadcaster, context) = init_test_context();

        let mut broadcast_receiver = broadcaster.subscribe();

        let mut use_case = PeriodicPNAdvertising::<_, 20>::new(Config {
            probing_timeout: Duration::from_secs(0),
        });

        assert!(use_case.start(&context).is_ok());

        let timer_id = match &use_case.state {
            State::Running(timer_id) => *timer_id,
            _ => panic!("Invalid state returned: {:?}", &use_case.state),
        };

        let event = broadcast_receiver
            .try_recv()
            .expect("failed to receive event");
        assert_eq!(event, UseCaseEvent::Timer(timer_id));
    }
}
