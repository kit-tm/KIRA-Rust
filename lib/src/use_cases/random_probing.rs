use std::error::Error;
use std::fmt::{Display, Formatter};
use std::marker::PhantomData;
use std::time::Duration;

use crate::context::UseCaseContext;
use crate::domain::NodeId;
use crate::messaging::{FindNodeReqData, Nonce, ProtocolMessageSender, ReqRspMessage};
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{TimerId, UseCase, UseCaseEvent, UseCaseState};

#[derive(Debug, Copy, Clone)]
pub struct RandomProbingConfig {
    pub timeout: Duration,
    pub neighborhood_size: u64,
}

impl Default for RandomProbingConfig {
    fn default() -> Self {
        Self {
            // Default: 2.5 Messages/s => 1000 ms / 2.5 = 400 ms
            timeout: Duration::from_millis(400),
            neighborhood_size: 20,
        }
    }
}

/// Probe a random [NodeId] to keep [Bucket]s up-to-date.
#[derive(Debug)]
pub struct RandomProbingUseCase<C> {
    _c: PhantomData<C>,
    config: RandomProbingConfig,
    state: RandomProbingState,
}

impl<C> RandomProbingUseCase<C> {
    pub fn new(config: RandomProbingConfig) -> Self {
        Self {
            _c: PhantomData::default(),
            config,
            state: RandomProbingState::Initialized,
        }
    }
}

impl<C> UseCase for RandomProbingUseCase<C>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    C::MessageSender: ProtocolMessageSender,
{
    type Context = C;
    type Error = RandomProbingError;
    type State = RandomProbingState;

    fn start(&mut self, context: &C) -> Result<(), Self::Error> {
        let timer_id = context
            .runtime()
            .register_periodic_timer(self.config.timeout);

        self.state = RandomProbingState::Running(timer_id);

        Ok(())
    }

    fn handle_event(&mut self, context: &C, event: UseCaseEvent) -> Result<(), Self::Error> {
        if let (UseCaseEvent::Timer(event_id), RandomProbingState::Running(timer_id)) =
            (event, &self.state)
        {
            if &event_id == timer_id {
                let random_id = NodeId::random();

                let message = ReqRspMessage {
                    nonce: Nonce::random(),
                    source: context.root_id().clone(),
                    destination: random_id,
                    data: FindNodeReqData { exact: false },
                };

                if let Err(e) = context.message_sender_mut().send(message) {
                    log::error!("MessageSender failed: {}", e);
                    self.state = RandomProbingState::Error;
                    return Err(RandomProbingError::SendFailed);
                }
            }
        }

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

#[derive(Debug)]
pub enum RandomProbingError {
    SendFailed,
    EmptyRoutingTable,
}

impl Display for RandomProbingError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SendFailed => write!(f, "Failed to send message"),
            Self::EmptyRoutingTable => write!(f, "Could not probe, routing table is empty"),
        }
    }
}

impl Error for RandomProbingError {}

#[derive(Debug, Eq, PartialEq)]
pub enum RandomProbingState {
    Initialized,
    Running(TimerId),
    Error,
}

impl UseCaseState for RandomProbingState {
    /// Random probing will never be finished.
    fn is_finished(&self) -> bool {
        false
    }

    fn is_error(&self) -> bool {
        self == &Self::Error
    }
}

#[cfg(all(test, feature = "bus"))]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use crate::broadcaster::BusBroadcaster;
    use crate::context::{SyncContext, UseCaseContext};
    use crate::domain::{
        Age, Contact, FlatRoutingTable, InsertionStrategyResult, NodeId, PNTable, Path,
        RoutingTable, StateSeqNr, TestInsertionStrategy,
    };
    use crate::messaging::tests::ArcSyncInMemoryMessageHub;
    use crate::messaging::ProtocolMessage;
    use crate::runtime::DummyRuntime;
    use crate::use_cases::random_probing::{
        RandomProbingConfig, RandomProbingState, RandomProbingUseCase,
    };
    use crate::use_cases::{UseCase, UseCaseEvent};

    fn init_test_context() -> (
        NodeId,
        ArcSyncInMemoryMessageHub,
        Arc<BusBroadcaster>,
        DummyRuntime<BusBroadcaster>,
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

        let runtime = DummyRuntime::new(Arc::clone(&broadcaster));

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let context = SyncContext::new(
            root.clone(),
            routing_table,
            PNTable::new(),
            insertion_strategy,
            hub.clone(),
            runtime.clone(),
        );

        (root, hub, broadcaster, runtime, context)
    }

    #[test]
    fn smoke_test() {
        // Tests if start succeeds and a periodic random id is probed

        let (root_id, hub, _broadcaster, runtime, context) = init_test_context();

        // Insert a single contact into the routing table
        let contact = Contact::new(
            NodeId::random(),
            Age::from(0),
            Path::empty(),
            StateSeqNr::from(0),
        );
        context
            .routing_table_mut()
            .add(contact.clone())
            .expect("failed to add into empty RT");

        let mut use_case = RandomProbingUseCase::new(RandomProbingConfig {
            timeout: Duration::from_secs(0),
            neighborhood_size: 0,
        });

        let start_result = use_case.start(&context);
        assert!(start_result.is_ok(), "{:?}", start_result);

        assert!(matches!(&use_case.state, RandomProbingState::Running(_)));
        let timer_id = match &use_case.state {
            RandomProbingState::Running(timer_id) => timer_id.clone(),
            state => panic!("Unexpected State: {:?}", state),
        };
        assert_ne!(runtime.counter(), 0);

        let handle_result = use_case.handle_event(&context, UseCaseEvent::Timer(timer_id));
        assert!(handle_result.is_ok(), "{:?}", handle_result);

        for _ in 0..2 {
            let message = hub.messages().into_iter().find_map(|message| {
                if let ProtocolMessage::FindNodeReq(msg) = message {
                    Some(msg)
                } else {
                    None
                }
            });
            assert!(message.is_some(), "No FindNodeReq emitted by use case");
            let message = message.unwrap();

            // Request is from Sender Node, random id and exact flag is set to false
            assert_eq!(&message.source, &root_id);
            assert!(!message.destination.is_zero());
            assert!(!message.data.exact);

            let handle_result = use_case.handle_event(&context, UseCaseEvent::Timer(timer_id));
            assert!(handle_result.is_ok(), "{:?}", handle_result);
        }
    }
}
