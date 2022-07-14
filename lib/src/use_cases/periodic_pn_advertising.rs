use std::fmt::{Display, Formatter};
use std::marker::PhantomData;
use std::time::Duration;

use crate::context::UseCaseContext;
use crate::messaging::{HelloMessage, ProtocolMessageSender};
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{TimerId, UseCase, UseCaseEvent, UseCaseState};

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub struct PeriodicPNAdvertisingConfig {
    pub probing_timeout: Duration,
}

impl Default for PeriodicPNAdvertisingConfig {
    fn default() -> Self {
        Self {
            // TODO: Useful timeout duration?
            probing_timeout: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum PeriodicPNAdvertisingState {
    Initialized,
    Running(TimerId),
    Error,
}

impl UseCaseState for PeriodicPNAdvertisingState {
    fn is_finished(&self) -> bool {
        false
    }

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
            .register_periodic_timer(self.config.probing_timeout);

        self.state = PeriodicPNAdvertisingState::Running(timer_id);

        Ok(())
    }

    fn handle_event(&mut self, context: &C, event: UseCaseEvent) -> Result<(), Self::Error> {
        if let (UseCaseEvent::Timer(id), PeriodicPNAdvertisingState::Running(timer_id)) =
            (event, self.state.clone())
        {
            if id == timer_id {
                if let Err(e) = context.message_sender_mut().send(HelloMessage {
                    source: context.root_id().clone(),
                    source_state_seq_nr: *context.pn_table().state_seq_nr(),
                }) {
                    log::error!("MessageSender failed: {}", e);
                    self.state = PeriodicPNAdvertisingState::Error;
                    return Err(PeriodicPNAdvertisingError::SendError);
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
    use std::time::Duration;

    use crate::broadcaster::{Broadcaster, BusBroadcaster};
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
        BusBroadcaster,
        SyncContext<
            FlatRoutingTable<20, 1>,
            ArcSyncInMemoryMessageHub,
            ImmediateRuntime<BusBroadcaster>,
            TestInsertionStrategy,
        >,
    ) {
        let root = NodeId::one();

        let routing_table = FlatRoutingTable::<20, 1>::new(root.clone())
            .expect("failed to build flat routing table");

        let hub = ArcSyncInMemoryMessageHub::new();

        let broadcaster = BusBroadcaster::new(1);

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let context = SyncContext::new(
            root.clone(),
            routing_table,
            PNTable::new(),
            insertion_strategy,
            hub.clone(),
            ImmediateRuntime::new(broadcaster.clone()),
        );

        (root, hub, broadcaster, context)
    }

    #[test]
    fn start_test() {
        let (_root_id, _hub, broadcaster, context) = init_test_context();

        let mut broadcast_receiver = broadcaster.subscribe();

        let mut use_case = PeriodicPNAdvertising::<_, 20>::new(PeriodicPNAdvertisingConfig {
            probing_timeout: Duration::from_secs(0),
        });

        assert!(use_case.start(&context).is_ok());

        let timer_id = match &use_case.state {
            PeriodicPNAdvertisingState::Running(timer_id) => *timer_id,
            _ => panic!("Invalid state returned: {:?}", &use_case.state),
        };

        let event = broadcast_receiver
            .try_recv()
            .expect("failed to receive event");
        assert_eq!(event, UseCaseEvent::Timer(timer_id));
    }
}
