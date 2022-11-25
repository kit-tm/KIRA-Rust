use std::error::Error;
use std::fmt::{Display, Formatter};
use std::marker::PhantomData;
use std::num::{NonZeroU64, NonZeroUsize};
use std::time::Duration;

use crate::context::UseCaseContext;
use crate::domain::{node_id, GroupingError, NodeId, RoutingTable};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{FindNodeReqData, Nonce, ProtocolMessageSender, ReqRspMessage};
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{EventHandler, TimerId, UseCase, UseCaseEvent, UseCaseState};

#[derive(Debug, Copy, Clone)]
pub struct RODConfig {
    pub timeout: Duration,
    pub neighborhood_size: NonZeroU64,
    pub shared_prefix_grouping: NonZeroUsize,
}

impl Default for RODConfig {
    fn default() -> Self {
        Self {
            // Default: 2.5 Messages/s => 1000 ms / 2.5 = 400 ms
            timeout: Duration::from_millis(400),
            neighborhood_size: NonZeroU64::new(20).unwrap(),
            shared_prefix_grouping: NonZeroUsize::new(1).unwrap(),
        }
    }
}

/// Probe a random [NodeId] to keep [Buckets](crate::domain::Bucket) up-to-date.
#[derive(Debug)]
pub struct RandomOverlayDiscovery<C, const BUCKET_SIZE: usize> {
    _c: PhantomData<C>,
    config: RODConfig,
    state: RODState,
}

impl<C, const BUCKET_SIZE: usize> RandomOverlayDiscovery<C, BUCKET_SIZE> {
    pub fn new(config: RODConfig) -> Result<Self, GroupingError> {
        if config.shared_prefix_grouping.get() > node_id::BIT_SIZE {
            return Err(GroupingError::Invalid {
                group_size: config.shared_prefix_grouping.get(),
                id_size: node_id::BIT_SIZE,
            });
        }

        Ok(Self {
            _c: PhantomData::default(),
            config,
            state: RODState::Initialized,
        })
    }
}

impl<C, const BUCKET_SIZE: usize> RandomOverlayDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    C::MessageSender: ProtocolMessageSender,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
{
    fn send_find_node_req(&mut self, context: &C) -> Result<(), RODError> {
        let random_id = NodeId::random();

        let closest_path = context
            .routing_table()
            .closest(
                &random_id,
                BUCKET_SIZE,
                self.config.shared_prefix_grouping.get(),
            )
            .expect("grouping was checked on initialization")
            .first()
            .map(|(_, contact)| contact.path())
            .cloned();
        if closest_path.is_none() {
            log::trace!(
                target: "random_overlay_discovery",
                "No closest contact found for random id; Assuming isolation"
            );
            return Ok(());
        }
        let closest_path = closest_path.unwrap();

        // Get interface of neighbor
        let interface = context.pn_table().get(closest_path.first()).cloned();
        if interface.is_none() {
            log::error!(
                target: "random_overlay_discovery",
                "Contacts path contains invalid neighbor: {}",
                closest_path
            );
            self.state = RODState::Error;
            return Err(RODError::InvalidNeighbor);
        }

        let mut route = SourceRoute::from(closest_path);
        route.push_front(context.root_id().clone());

        let message = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
            data: FindNodeReqData {
                exact: false,
                neighborhood: self.config.neighborhood_size,
                target: random_id,
            },
            not_via: context.not_via().clone(),
            source_route: route,
        };
        log::trace!(target: "random_overlay_discovery", "Sending message {:?}", message);

        if let Err(e) = context.message_sender_mut().send_message(message) {
            log::error!("MessageSender failed: {}", e);
            return Err(RODError::SendFailed);
        }

        Ok(())
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase for RandomOverlayDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    C::MessageSender: ProtocolMessageSender,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
{
    type State = RODState;

    fn start(&mut self, context: &C) -> Result<(), Self::Error> {
        let timer_id = context
            .runtime()
            .register_periodic_timer(self.config.timeout);

        self.state = RODState::Running(timer_id);

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

impl<C, const BUCKET_SIZE: usize> EventHandler for RandomOverlayDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    C::MessageSender: ProtocolMessageSender,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
{
    type Context = C;
    type Error = RODError;
    type Value = ();

    fn handle_event(&mut self, context: &C, event: UseCaseEvent) -> Result<(), Self::Error> {
        if let (UseCaseEvent::Timer(event_id), RODState::Running(timer_id)) =
            (event, &mut self.state)
        {
            if &event_id != timer_id {
                return Ok(());
            }

            self.send_find_node_req(context)?;
        }

        Ok(())
    }
}

#[derive(Debug)]
pub enum RODError {
    SendFailed,
    EmptyRoutingTable,
    InvalidNeighbor,
}

impl Display for RODError {
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

impl Error for RODError {}

#[derive(Debug, Eq, PartialEq)]
pub enum RODState {
    Initialized,
    Running(TimerId),
    Error,
}

impl UseCaseState for RODState {
    fn is_error(&self) -> bool {
        self == &Self::Error
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::num::{NonZeroU64, NonZeroUsize};
    use std::time::Duration;

    use crate::broadcaster::MPSCBroadcaster;
    use crate::context::{ContextConfig, SyncContext, UseCaseContext};
    use crate::domain::{
        Contact, FlatRoutingTable, InsertionStrategyResult, NetworkInterface, NodeId, PNTable,
        Path, RoutingTable, StateSeqNr, TestInsertionStrategy,
    };
    use crate::forwarding::in_memory_tables::InMemoryFwdTables;
    use crate::messaging::{
        AsyncProtocolMessageReceiver, InMemoryMessageChannel, ProtocolMessage,
        ProtocolMessageSender,
    };
    use crate::runtime::ImmediateRuntime;
    use crate::use_cases::random_overlay_discovery::{RODConfig, RODState, RandomOverlayDiscovery};
    use crate::use_cases::{EventHandler, UseCase, UseCaseEvent};

    fn init_test_context() -> (
        NodeId,
        impl AsyncProtocolMessageReceiver,
        MPSCBroadcaster,
        ImmediateRuntime<MPSCBroadcaster>,
        SyncContext<
            FlatRoutingTable<20, 1>,
            impl ProtocolMessageSender,
            ImmediateRuntime<MPSCBroadcaster>,
            TestInsertionStrategy,
            InMemoryFwdTables,
        >,
    ) {
        let root = NodeId::one();

        let routing_table = FlatRoutingTable::<20, 1>::new(root.clone())
            .expect("failed to build flat routing table");

        let (hub_sender, hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster.clone());

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let fwd_tables = InMemoryFwdTables::new();

        let context = SyncContext::new(ContextConfig {
            root_id: root.clone(),
            routing_table,
            pn_table: PNTable::new(),
            insertion_strategy,
            message_sender: hub_sender,
            runtime: runtime.clone(),
            forwarding_tables: fwd_tables,
            not_via: HashSet::default(),
        });

        (root, hub_receiver, broadcaster, runtime, context)
    }

    #[tokio::test]
    async fn smoke_test() {
        // Tests if start succeeds and a periodic random id is probed

        let (root_id, mut hub, _broadcaster, _runtime, context) = init_test_context();

        // Insert a single contact into the routing table
        let neighbor = Contact::new(Path::from(NodeId::random()), StateSeqNr::from(0));
        context
            .routing_table_mut()
            .add(neighbor.clone())
            .expect("failed to add into empty RT");
        context
            .pn_table_mut()
            .insert(neighbor.id().clone(), NetworkInterface::new("test"));

        let mut use_case = RandomOverlayDiscovery::new(RODConfig {
            timeout: Duration::from_secs(0),
            neighborhood_size: NonZeroU64::new(20).unwrap(),
            shared_prefix_grouping: NonZeroUsize::new(1).unwrap(),
        })
        .unwrap();

        let start_result = use_case.start(&context);
        assert!(start_result.is_ok(), "{:?}", start_result);

        assert!(matches!(&use_case.state, RODState::Running { .. }));
        let timer_id = match &use_case.state {
            RODState::Running(timer_id) => *timer_id,
            state => panic!("Unexpected State: {:?}", state),
        };

        let handle_result = use_case.handle_event(&context, UseCaseEvent::Timer(timer_id));
        assert!(handle_result.is_ok(), "{:?}", handle_result);

        let messages = {
            let mut messages = Vec::new();
            while let Ok(Some((message, _))) = hub.try_recv().await {
                messages.push(message);
            }
            messages
        };

        for _ in 0..2 {
            let message = messages.iter().find_map(|message| {
                if let ProtocolMessage::FindNodeReq(msg) = message {
                    Some(msg.clone())
                } else {
                    None
                }
            });
            assert!(message.is_some(), "No FindNodeReq emitted by use case");
            let message = message.unwrap();

            // Request is from Sender Node, random id and exact flag is set to false
            assert_eq!(message.source(), &root_id);
            assert!(!message.destination().is_zero());
            assert!(!message.data.exact);

            assert_eq!(
                message.source_route.current_hop(),
                neighbor.id(),
                "Neighbor should be the current hop"
            );

            let handle_result = use_case.handle_event(&context, UseCaseEvent::Timer(timer_id));
            assert!(handle_result.is_ok(), "{:?}", handle_result);
        }
    }
}
