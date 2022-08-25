use std::error::Error;
use std::fmt::{Display, Formatter};
use std::marker::PhantomData;
use std::num::{NonZeroU64, NonZeroUsize};
use std::time::Duration;

use crate::context::UseCaseContext;
use crate::domain::{NodeId, RoutingTable};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{FindNodeReqData, Nonce, ProtocolMessageSender, ReqRspMessage};
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{TimerId, UseCase, UseCaseEvent, UseCaseState};

#[derive(Debug, Copy, Clone)]
pub struct RandomProbingConfig {
    pub timeout: Duration,
    pub neighborhood_size: NonZeroU64,
    pub shared_prefix_grouping: NonZeroUsize,
}

impl Default for RandomProbingConfig {
    fn default() -> Self {
        Self {
            // Default: 2.5 Messages/s => 1000 ms / 2.5 = 400 ms
            timeout: Duration::from_millis(400),
            neighborhood_size: NonZeroU64::new(20).unwrap(),
            shared_prefix_grouping: NonZeroUsize::new(1).unwrap(),
        }
    }
}

/// Probe a random [NodeId] to keep [Bucket]s up-to-date.
#[derive(Debug)]
pub struct RandomProbingUseCase<C, const BUCKET_SIZE: usize> {
    _c: PhantomData<C>,
    config: RandomProbingConfig,
    state: RandomProbingState,
}

impl<C, const BUCKET_SIZE: usize> RandomProbingUseCase<C, BUCKET_SIZE> {
    pub fn new(config: RandomProbingConfig) -> Self {
        Self {
            _c: PhantomData::default(),
            config,
            state: RandomProbingState::Initialized,
        }
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase for RandomProbingUseCase<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    C::MessageSender: ProtocolMessageSender,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
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

                let closest_path = context
                    .routing_table()
                    .get_closest(&random_id, self.config.shared_prefix_grouping.get())
                    .map(|contact| contact.path())
                    .cloned();
                if closest_path.is_none() {
                    log::warn!("No closest contact found for random id; Assuming isolation");
                    return Ok(());
                }
                let closest_path = closest_path.unwrap();

                // Get port of neighbor
                let port = context.pn_table().get(closest_path.first()).cloned();
                if port.is_none() {
                    log::error!("Contacts path contains invalid neighbor: {}", closest_path);
                    self.state = RandomProbingState::Error;
                    return Err(RandomProbingError::InvalidNeighbor);
                }
                let port = port.unwrap();

                let message = ReqRspMessage {
                    nonce: Nonce::random(),
                    source: context.root_id().clone(),
                    target: random_id,
                    data: FindNodeReqData {
                        exact: false,
                        neighborhood: self.config.neighborhood_size,
                    },
                    source_route: SourceRoute::from(closest_path),
                };

                if let Err(e) = context.message_sender_mut().send(message, port) {
                    log::error!("MessageSender failed: {}", e);
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
    InvalidNeighbor,
}

impl Display for RandomProbingError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SendFailed => write!(f, "Failed to send message"),
            Self::EmptyRoutingTable => write!(f, "Could not probe, routing table is empty"),
            Self::InvalidNeighbor => {
                write!(f, "The path of a contact contains an invalid neighbor")
            }
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
    fn is_error(&self) -> bool {
        self == &Self::Error
    }
}

#[cfg(test)]
mod tests {
    use std::num::{NonZeroU64, NonZeroUsize};
    use std::time::Duration;

    use crate::broadcaster::MPSCBroadcaster;
    use crate::context::{SyncContext, UseCaseContext};
    use crate::domain::{
        Age, Contact, FlatRoutingTable, InsertionStrategyResult, NodeId, PNTable, Path, Port,
        RoutingTable, StateSeqNr, TestInsertionStrategy,
    };
    use crate::messaging::tests::ArcSyncInMemoryMessageHub;
    use crate::messaging::ProtocolMessage;
    use crate::runtime::ImmediateRuntime;
    use crate::use_cases::random_probing::{
        RandomProbingConfig, RandomProbingState, RandomProbingUseCase,
    };
    use crate::use_cases::{UseCase, UseCaseEvent};

    fn init_test_context() -> (
        NodeId,
        ArcSyncInMemoryMessageHub,
        MPSCBroadcaster,
        ImmediateRuntime<MPSCBroadcaster>,
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

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster.clone());

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

        let (root_id, hub, _broadcaster, _runtime, context) = init_test_context();

        // Insert a single contact into the routing table
        let contact = Contact::new(
            Path::from(NodeId::random()),
            Age::from(0),
            StateSeqNr::from(0),
        );
        context
            .routing_table_mut()
            .add(contact.clone())
            .expect("failed to add into empty RT");
        context
            .pn_table_mut()
            .add(contact.id().clone(), Port::Named(String::from("test")));

        let mut use_case = RandomProbingUseCase::new(RandomProbingConfig {
            timeout: Duration::from_secs(0),
            neighborhood_size: NonZeroU64::new(20).unwrap(),
            shared_prefix_grouping: NonZeroUsize::new(1).unwrap(),
        });

        let start_result = use_case.start(&context);
        assert!(start_result.is_ok(), "{:?}", start_result);

        assert!(matches!(&use_case.state, RandomProbingState::Running(_)));
        let timer_id = match &use_case.state {
            RandomProbingState::Running(timer_id) => *timer_id,
            state => panic!("Unexpected State: {:?}", state),
        };

        let handle_result = use_case.handle_event(&context, UseCaseEvent::Timer(timer_id));
        assert!(handle_result.is_ok(), "{:?}", handle_result);

        for _ in 0..2 {
            let message = hub.messages().into_iter().find_map(|message| {
                if let ProtocolMessage::FindNodeReq(msg) = message.0 {
                    Some(msg)
                } else {
                    None
                }
            });
            assert!(message.is_some(), "No FindNodeReq emitted by use case");
            let message = message.unwrap();

            // Request is from Sender Node, random id and exact flag is set to false
            assert_eq!(&message.source, &root_id);
            assert!(!message.target.is_zero());
            assert!(!message.data.exact);

            let handle_result = use_case.handle_event(&context, UseCaseEvent::Timer(timer_id));
            assert!(handle_result.is_ok(), "{:?}", handle_result);
        }
    }
}
