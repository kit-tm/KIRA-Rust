use std::error::Error;
use std::fmt::{Display, Formatter};
use std::marker::PhantomData;
use std::time::Duration;

use crate::context::Context;
use crate::domain::NodeId;
use crate::messaging::{HelloMessage, ProtocolMessageSender};
use crate::runtime::Runtime;
use crate::use_cases::{TimerId, UseCase, UseCaseEvent, UseCaseState};

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub struct PNProbingConfig {
    pub probing_timeout: Duration,
}

impl Default for PNProbingConfig {
    fn default() -> Self {
        Self {
            // TODO: Useful timeout duration?
            probing_timeout: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum PNProbingState {
    Initialized,
    Running(TimerId),
    Error,
}

impl UseCaseState for PNProbingState {
    fn is_finished(&self) -> bool {
        false
    }

    fn is_error(&self) -> bool {
        self == &Self::Error
    }
}

#[derive(Debug)]
pub enum PNProbingError {
    SendError,
}

impl Display for PNProbingError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SendError => write!(f, "Sending a message failed"),
        }
    }
}

impl Error for PNProbingError {}

#[derive(Debug, Clone)]
pub struct PNProbingUseCase<C, const BUCKET_SIZE: usize> {
    _pd: PhantomData<C>,
    state: PNProbingState,
    config: PNProbingConfig,
}

impl<C, const BUCKET_SIZE: usize> Default for PNProbingUseCase<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self::new(PNProbingConfig::default())
    }
}

impl<C, const BUCKET_SIZE: usize> PNProbingUseCase<C, BUCKET_SIZE> {
    pub fn new(config: PNProbingConfig) -> Self {
        Self {
            _pd: PhantomData::default(),
            state: PNProbingState::Initialized,
            config,
        }
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase for PNProbingUseCase<C, BUCKET_SIZE>
where
    C: Context,
    C::Runtime: Runtime,
    C::MessageSender: ProtocolMessageSender,
{
    type Context = C;
    type Error = PNProbingError;
    type State = PNProbingState;

    fn start(&mut self, context: &C) -> Result<(), Self::Error> {
        let timer_id = context
            .runtime()
            .register_periodic_timer(self.config.probing_timeout);

        self.state = PNProbingState::Running(timer_id);

        Ok(())
    }

    fn handle_event(&mut self, context: &C, event: UseCaseEvent) -> Result<(), Self::Error> {
        if let (UseCaseEvent::Timer(id), PNProbingState::Running(timer_id)) =
            (event, self.state.clone())
        {
            if id == timer_id {
                if let Err(e) = context.message_sender_mut().send(HelloMessage {
                    source: context.root_id().clone(),
                    destination: NodeId::zero(),
                }) {
                    log::error!("MessageSender failed: {}", e);
                    self.state = PNProbingState::Error;
                    return Err(PNProbingError::SendError);
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
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    use crate::broadcaster::{Broadcaster, BusBroadcaster};
    use crate::context::SyncContext;
    use crate::domain::{FlatRoutingTable, NodeId, Port};
    use crate::messaging::tests::ArcSyncInMemoryMessageHub;
    use crate::runtime::DummyRuntime;
    use crate::use_cases::pn_probing::{PNProbingConfig, PNProbingState, PNProbingUseCase};
    use crate::use_cases::{UseCase, UseCaseEvent};

    fn init_test_context() -> (
        NodeId,
        ArcSyncInMemoryMessageHub,
        Arc<BusBroadcaster>,
        SyncContext<
            FlatRoutingTable<20, 1>,
            HashMap<NodeId, Port>,
            ArcSyncInMemoryMessageHub,
            DummyRuntime<BusBroadcaster>,
        >,
    ) {
        let root = NodeId::one();

        let routing_table = FlatRoutingTable::<20, 1>::new(root.clone())
            .expect("failed to build flat routing table");

        let hub = ArcSyncInMemoryMessageHub::new();

        let broadcaster = Arc::new(BusBroadcaster::new(1));

        let context = SyncContext::new(
            root.clone(),
            routing_table,
            HashMap::<NodeId, Port>::new(),
            hub.clone(),
            DummyRuntime::new(Arc::clone(&broadcaster)),
        );

        (root, hub, broadcaster, context)
    }

    #[test]
    fn start_test() {
        let (_root_id, _hub, broadcaster, context) = init_test_context();

        let mut broadcast_receiver = broadcaster.subscribe();

        let mut use_case = PNProbingUseCase::<_, 20>::new(PNProbingConfig {
            probing_timeout: Duration::from_secs(0),
        });

        assert!(use_case.start(&context).is_ok());

        let timer_id = match &use_case.state {
            PNProbingState::Running(timer_id) => *timer_id,
            _ => panic!("Invalid state returned: {:?}", &use_case.state),
        };

        let event = broadcast_receiver
            .try_recv()
            .expect("failed to receive event");
        assert_eq!(event, UseCaseEvent::Timer(timer_id));
    }
}
