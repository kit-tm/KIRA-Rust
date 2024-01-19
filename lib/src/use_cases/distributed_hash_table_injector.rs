use std::fmt::Debug;
use core::time::Duration;
use std::collections::{HashMap, LinkedList};
use std::marker::PhantomData;
use std::num::NonZeroUsize;
use std::time::Instant;

use crate::context::UseCaseContext;
use crate::domain::{GroupingError, node_id, PNTable, RoutingTable};
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{EventHandler, FetchInjectData, InjectionMessageData, OneshotInjectMessageCallback, StoreInjectData, TimerId, UseCase, UseCaseEvent, UseCaseState};

use crate::messaging::{Nonce, ProtocolMessage, ProtocolMessageSender, ReqRspMessage};
use crate::messaging::dht::{DefaultLHTInput, FetchReqData, StoreReqData};
use crate::messaging::source_route::SourceRoute;
use crate::use_cases::distributed_hash_table_injector::DHTInjectorState::Running;
use crate::use_cases::inject_messages::InjectionResult;
use crate::use_cases::inject_messages::errors::InjectMessageError;

// todo what would be a sensible value here?
// todo random offsets for periodic restore AND collection of the hash table?
pub const DEFAULT_PERIODIC_RESTORE: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct DistributedHashTableInjectorConfig
{
    periodic_restore: Duration,
    shared_prefix_grouping: NonZeroUsize,
}

impl Default for DistributedHashTableInjectorConfig {
    fn default() -> Self {
        Self {
            periodic_restore: DEFAULT_PERIODIC_RESTORE,
            shared_prefix_grouping: NonZeroUsize::new(1).unwrap(),
        }
    }
}

#[derive(Debug, Eq, PartialEq, Clone, Default)]
pub enum DHTInjectorState {
    #[default]
    Initialized,
    Running(TimerId),
    Error,
}

pub struct DistributedHashTableInjector<C, const BUCKET_SIZE: usize>
{
    _c: PhantomData<C>,
    state: DHTInjectorState,
    config: DistributedHashTableInjectorConfig,
    nonces: HashMap<Nonce, (Instant, OneshotInjectMessageCallback)>,
    restore_data: LinkedList<StoreReqData<DefaultLHTInput>>,
}

impl UseCaseState for DHTInjectorState {
    fn is_error(&self) -> bool {
        self == &Self::Error
    }
}

impl<C, const BUCKET_SIZE: usize> DistributedHashTableInjector<C, BUCKET_SIZE>
{
    pub fn new(config: DistributedHashTableInjectorConfig) -> Result<Self, GroupingError> {
        if config.shared_prefix_grouping.get() > node_id::BIT_SIZE {
            return Err(GroupingError::Invalid {
                group_size: config.shared_prefix_grouping.get(),
                id_size: node_id::BIT_SIZE,
            });
        }

        Ok(Self {
            _c: PhantomData,
            state: DHTInjectorState::default(),
            config,
            nonces: HashMap::default(),
            restore_data: LinkedList::default(),
        })
    }
}

impl<C, const BUCKET_SIZE: usize> Default for DistributedHashTableInjector<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self::new(DistributedHashTableInjectorConfig::default())
            .expect("default grouping has to be valid")
    }
}

impl<C, const BUCKET_SIZE: usize, D: Debug> DistributedHashTableInjector<C, BUCKET_SIZE>
    where
        C: UseCaseContext,
        for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
        C::Runtime: UseCaseRuntime<SendError=D>,
        C::PhysicalNeighborTable: PNTable
{
    fn construct_req_rsp_msg<T: Debug>(context: &C, nonce: Nonce, data: T) -> ReqRspMessage<T> {
        let mut source_route = SourceRoute::from(context.root_id().clone());
        source_route.push_front(context.root_id().clone());

        ReqRspMessage {
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
            not_via: context.not_via().clone(),
            data,
            nonce,
            source_route,
        }
    }

    fn send_store_req(&self, context: &C, nonce: Nonce, data: StoreReqData<DefaultLHTInput>) -> Result<(), InjectMessageError> {
        let message = Self::construct_req_rsp_msg(context, nonce, data);

        log::trace!(target: "inject_dht_messages", "Sending StoreReq from {} with destination {}",
            message.source_route.source(),
            message.data.handle
        );

        let message = ProtocolMessage::StoreReq(message);
        context.runtime().send_message(message).map_err(|e| {
            log::error!(target: "inject_dht_messages", "Failed to send triggered message: {:?}", e);
            InjectMessageError::SendFailed
        })
    }

    fn send_fetch_req(&self, context: &C, nonce: Nonce, data: FetchReqData) -> Result<(), InjectMessageError> {
        let message = Self::construct_req_rsp_msg(context, nonce, data);

        log::trace!(target: "inject_dht_messages", "Sending FetchReq from {} with destination {}",
            message.source_route.source(),
            message.data.handle
        );

        let message = ProtocolMessage::FetchReq(message);
        context.runtime().send_message(message).map_err(|e| {
            log::error!(target: "inject_dht_messages", "Failed to send triggered message: {:?}", e);
            InjectMessageError::SendFailed
        })
    }
}

impl<C, const BUCKET_SIZE: usize> DistributedHashTableInjector<C, BUCKET_SIZE> {
    fn send_inject_result(&self, result: InjectionResult, callback: OneshotInjectMessageCallback) -> Result<(), InjectMessageError> {
        callback.send(result).map_err(|e| {
            log::error!(target: "inject_dht_messages", "failed to send inject result: {:#?}", e);

            InjectMessageError::SendResultFailed
        })
    }

    fn inform_injector_about_send_error(&self, injection_error: InjectMessageError, nonce: &Nonce, callback: OneshotInjectMessageCallback) -> Result<(), InjectMessageError> {
        match injection_error {
            InjectMessageError::Isolated => {
                self.send_inject_result(InjectionResult::Isolated, callback)?
            }
            InjectMessageError::SendFailed => {
                self.send_inject_result(InjectionResult::SendFailed(nonce.clone()), callback)?
            }
            InjectMessageError::SendResultFailed => {
                // no information since the cause was that we couldn't reach the injector
                return Err(InjectMessageError::SendResultFailed);
            }
        }

        Ok(())
    }
}


impl<C, const BUCKET_SIZE: usize, D: Debug> EventHandler for DistributedHashTableInjector<C, BUCKET_SIZE>
    where
        C: UseCaseContext,
        for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
        C::MessageSender: ProtocolMessageSender,
        C::Runtime: UseCaseRuntime<SendError=D>,
        C::PhysicalNeighborTable: PNTable
{
    type Context = C;
    type Error = InjectMessageError;
    type Value = ();

    fn handle_event(&mut self, context: &Self::Context, event: UseCaseEvent) -> Result<Self::Value, Self::Error> {
        match (event, &self.state) {
            (UseCaseEvent::InjectMessage(nonce, InjectionMessageData::Store(StoreInjectData { handle, data, restore }, callback)), _) => {
                let payload = StoreReqData {
                    handle,
                    data,
                };

                if let Err(inject_err) = self.send_store_req(context, nonce.clone(), payload.clone()) {
                    self.inform_injector_about_send_error(inject_err, &nonce, callback)?;
                } else {
                    self.nonces.insert(nonce.clone(), (Instant::now(), callback));
                    if restore {
                        self.restore_data.push_back(payload);
                    }
                }
            }
            (UseCaseEvent::InjectMessage(nonce, InjectionMessageData::Fetch(FetchInjectData { handle }, callback)), _) => {
                let payload = FetchReqData {
                    handle,
                };

                if let Err(inject_err) = self.send_fetch_req(context, nonce.clone(), payload) {
                    self.inform_injector_about_send_error(inject_err, &nonce, callback)?;
                } else {
                    self.nonces.insert(nonce.clone(), (Instant::now(), callback.clone()));
                }
            }
            (UseCaseEvent::Message(message, interface), _) => {
                // don't react on looped requests
                match message {
                    ProtocolMessage::StoreReq(_) | ProtocolMessage::FetchReq(_) => return Ok(()),
                    _ => {}
                }

                if let Some(Some((instant, callback))) = message.nonce().map(|nonce| self.nonces.remove(nonce))
                {
                    let elapsed = instant.elapsed();
                    self.send_inject_result(InjectionResult::Answered((message.clone(), interface)), callback)?;

                    log::trace!(target: "inject_dht_messages", "Received response for nonce {:?} after {:?}", message.nonce(), elapsed);
                }
            }
            (UseCaseEvent::Timer(id), Running(our_id)) => {
                if &id == our_id {
                    for data in self.restore_data.iter() {
                        // todo make this more efficient
                        // 1.) calculate source routes only once for every handle
                        // 2.) pack all data to that node into a single request
                        self.send_store_req(context, Nonce::random(), data.clone())?;
                    }
                }
            }
            _ => {}
        }

        Ok(())
    }
}


impl<C, const BUCKET_SIZE: usize> UseCase for DistributedHashTableInjector<C, BUCKET_SIZE>
    where
        C: UseCaseContext,
        for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
        C::MessageSender: ProtocolMessageSender,
        C::Runtime: UseCaseRuntime,
        C::PhysicalNeighborTable: PNTable
{
    type State = DHTInjectorState;

    fn start(&mut self, context: &Self::Context) -> Result<(), Self::Error> {
        let timer_id = context.runtime().register_periodic_timer(self.config.periodic_restore);

        self.state = Running(timer_id);

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::sync::Arc;
    use std::time::Instant;
    use crate::broadcaster::MPSCBroadcaster;
    use crate::context::{ContextConfig, SyncContext, UseCaseContext};
    use crate::domain::{InMemoryPNTable, InsertionStrategyResult, NetworkInterface, NodeId, StateSeqNr, TestInsertionStrategy};
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::forwarding::in_memory_tables::InMemoryFwdTables;
    use crate::messaging::{InMemoryMessageChannel, Nonce, ProtocolMessage, ReqRspMessage};
    use crate::messaging::dht::{DefaultLHTInput, FetchReqData, FetchRspData, StoreOK, StoreReqData, StoreRspData};
    use crate::messaging::source_route::SourceRoute;
    use crate::runtime::ImmediateRuntime;
    use crate::use_cases;
    use crate::use_cases::distributed_hash_table_injector::DistributedHashTableInjector;
    use crate::use_cases::{EventHandler, FetchInjectData, InjectionMessageData, StoreInjectData, UseCase, UseCaseEvent};
    use crate::use_cases::distributed_hash_table_injector::DHTInjectorState::Running;
    use crate::use_cases::inject_messages::InjectionResult;

    #[test]
    fn startup_test() {
        let root_id = NodeId::one();

        let routing_table = SingleBucketRT::<20>::new(root_id.clone());

        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::with_interface(NetworkInterface::with_name("test")).into_parts();

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(10);

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table: InMemoryPNTable::new(),
            insertion_strategy,
            message_sender: hub_sender,
            runtime: ImmediateRuntime::new(broadcaster.clone()),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case = DistributedHashTableInjector::default();

        assert!(use_case.start(&context).is_ok());
    }

    #[test]
    fn inject_store_req() {
        crate::tests::init();

        let root_id = NodeId::with_msb(0);

        let routing_table = SingleBucketRT::<20>::new(root_id.clone());

        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::with_interface(NetworkInterface::with_name("test")).into_parts();

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(10);

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table: InMemoryPNTable::new(),
            insertion_strategy,
            message_sender: hub_sender,
            runtime: ImmediateRuntime::new(broadcaster.clone()),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case = DistributedHashTableInjector::default();

        let inject_event = UseCaseEvent::InjectMessage(
            Nonce::from(1),
            InjectionMessageData::Store(
                StoreInjectData {
                    handle: NodeId::with_msb(1),
                    data:Arc::new([]),
                    restore: false,
                },
                tokio::sync::mpsc::unbounded_channel().0),
        );

        let handle_result = use_case.handle_event(&context, inject_event);
        assert!(
            handle_result.is_ok(),
            "Handling inserting a StoreReq returned error: {:?}",
            handle_result
        )
    }

    #[test]
    fn received_store_answer_gets_returned() {
        crate::tests::init();

        let root_id = NodeId::with_msb(0);
        let interface = NetworkInterface::with_name("test");

        let routing_table = SingleBucketRT::<20>::new(root_id.clone());

        let (hub_sender, _hub_receiver) =
            InMemoryMessageChannel::with_interface(interface.clone()).into_parts();

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(10);

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table: InMemoryPNTable::new(),
            insertion_strategy,
            message_sender: hub_sender,
            runtime: ImmediateRuntime::new(broadcaster.clone()),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });


        let mut use_case = DistributedHashTableInjector::default();

        let nonce = Nonce::from(1);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        use_case.nonces = HashMap::with_capacity(1);
        use_case.nonces.insert(nonce.clone(), (Instant::now(), tx));

        let response = ProtocolMessage::StoreRsp(
            ReqRspMessage {
                nonce,
                source_state_seq_nr: StateSeqNr::from(0),
                data: StoreRspData {
                    status: Ok(StoreOK::Created),
                },
                not_via: Default::default(),
                source_route: SourceRoute::from(root_id.clone()),
            }
        );

        let inject_event = UseCaseEvent::Message(response.clone(), interface.clone());

        let handle_result = use_case.handle_event(&context, inject_event);
        assert!(
            handle_result.is_ok(),
            "Handling StoreRsp returned error: {:?}",
            handle_result
        );

        let result = rx.try_recv();
        assert!(result.is_ok(), "No result sent: {:?}", result);
        let result = result.unwrap();
        assert_eq!(
            result,
            InjectionResult::Answered((response.clone(), interface.clone()))
        );
    }

    #[test]
    fn dont_return_store_req_as_answer() {
        crate::tests::init();

        let root_id = NodeId::with_msb(0);
        let interface = NetworkInterface::with_name("test");

        let routing_table = SingleBucketRT::<20>::new(root_id.clone());

        let (hub_sender, _hub_receiver) =
            InMemoryMessageChannel::with_interface(interface.clone()).into_parts();

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(10);

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table: InMemoryPNTable::new(),
            insertion_strategy,
            message_sender: hub_sender,
            runtime: ImmediateRuntime::new(broadcaster.clone()),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case = DistributedHashTableInjector::default();

        let nonce = Nonce::from(1);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        use_case.nonces = HashMap::with_capacity(1);
        use_case.nonces.insert(nonce.clone(), (Instant::now(), tx));

        let store_req = ProtocolMessage::StoreReq(
            ReqRspMessage {
                nonce,
                source_state_seq_nr: StateSeqNr::from(0),
                data: StoreReqData {
                    handle: root_id.clone(),
                    data: Arc::new([]),
                },
                not_via: Default::default(),
                source_route: SourceRoute::from(root_id.clone()),
            }
        );

        let handle_result = use_case.handle_event(&context, UseCaseEvent::Message(store_req, interface.clone()));
        assert!(
            handle_result.is_ok(),
            "Handling StoreReq returned error: {:?}",
            handle_result
        );

        let result = rx.try_recv();
        assert!(result.is_err(), "Result sent: {:?}", result);
    }

    #[test]
    fn inject_fetch_req() {
        crate::tests::init();

        let root_id = NodeId::with_msb(0);

        let routing_table = SingleBucketRT::<20>::new(root_id.clone());

        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::with_interface(NetworkInterface::with_name("test")).into_parts();

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(10);

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table: InMemoryPNTable::new(),
            insertion_strategy,
            message_sender: hub_sender,
            runtime: ImmediateRuntime::new(broadcaster.clone()),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case = DistributedHashTableInjector::default();

        let inject_event = UseCaseEvent::InjectMessage(
            Nonce::from(1),
            InjectionMessageData::Fetch(
                FetchInjectData {
                    handle: root_id.clone(),
                },
                tokio::sync::mpsc::unbounded_channel().0),
        );

        let handle_result = use_case.handle_event(&context, inject_event);
        assert!(
            handle_result.is_ok(),
            "Handling inserting a StoreReq returned error: {:?}",
            handle_result
        )
    }

    #[test]
    fn received_fetch_answer_gets_returned() {
        crate::tests::init();

        let root_id = NodeId::with_msb(0);
        let interface = NetworkInterface::with_name("test");

        let routing_table = SingleBucketRT::<20>::new(root_id.clone());

        let (hub_sender, _hub_receiver) =
            InMemoryMessageChannel::with_interface(interface.clone()).into_parts();

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(10);

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table: InMemoryPNTable::new(),
            insertion_strategy,
            message_sender: hub_sender,
            runtime: ImmediateRuntime::new(broadcaster.clone()),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });


        let mut use_case = DistributedHashTableInjector::default();

        let nonce = Nonce::from(1);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        use_case.nonces = HashMap::with_capacity(1);
        use_case.nonces.insert(nonce.clone(), (Instant::now(), tx));

        let response = ProtocolMessage::FetchRsp(
            ReqRspMessage {
                nonce,
                source_state_seq_nr: StateSeqNr::from(0),
                data: FetchRspData {
                    data: Ok(vec![Arc::new([])]),
                },
                not_via: Default::default(),
                source_route: SourceRoute::from(root_id.clone()),
            }
        );

        let inject_event = UseCaseEvent::Message(response.clone(), interface.clone());

        let handle_result = use_case.handle_event(&context, inject_event);
        assert!(
            handle_result.is_ok(),
            "Handling FetchRsp returned error: {:?}",
            handle_result
        );

        let result = rx.try_recv();
        assert!(result.is_ok(), "No result sent: {:?}", result);
        let result = result.unwrap();
        assert_eq!(
            result,
            InjectionResult::Answered((response.clone(), interface.clone()))
        );
    }

    #[test]
    fn dont_return_fetch_req_as_answer() {
        crate::tests::init();

        let root_id = NodeId::with_msb(0);
        let interface = NetworkInterface::with_name("test");

        let routing_table = SingleBucketRT::<20>::new(root_id.clone());

        let (hub_sender, _hub_receiver) =
            InMemoryMessageChannel::with_interface(interface.clone()).into_parts();

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(10);

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table: InMemoryPNTable::new(),
            insertion_strategy,
            message_sender: hub_sender,
            runtime: ImmediateRuntime::new(broadcaster.clone()),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case = DistributedHashTableInjector::default();

        let nonce = Nonce::from(1);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        use_case.nonces = HashMap::with_capacity(1);
        use_case.nonces.insert(nonce.clone(), (Instant::now(), tx));

        let store_req = ProtocolMessage::FetchReq(
            ReqRspMessage {
                nonce,
                source_state_seq_nr: StateSeqNr::from(0),
                data: FetchReqData {
                    handle: root_id.clone(),
                },
                not_via: Default::default(),
                source_route: SourceRoute::from(root_id.clone()),
            }
        );

        let handle_result = use_case.handle_event(&context, UseCaseEvent::Message(store_req, interface.clone()));
        assert!(
            handle_result.is_ok(),
            "Handling FetchReq returned error: {:?}",
            handle_result
        );

        let result = rx.try_recv();
        assert!(result.is_err(), "Result sent: {:?}", result);
    }

    #[tokio::test]
    async fn restoring_data() {
        crate::tests::init();

        let root_id = NodeId::with_msb(0);
        let interface = NetworkInterface::with_name("test");

        let routing_table = SingleBucketRT::<20>::new(root_id.clone());

        let (hub_sender, _hub_receiver) =
            InMemoryMessageChannel::with_interface(interface.clone()).into_parts();

        let (broadcaster, broadcast_receiver) = MPSCBroadcaster::new(10);

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table: InMemoryPNTable::new(),
            insertion_strategy,
            message_sender: hub_sender,
            runtime: ImmediateRuntime::new(broadcaster.clone()),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case = DistributedHashTableInjector::default();

        let restore_data: StoreReqData<DefaultLHTInput> = StoreReqData {
            handle: root_id.clone(),
            data: Arc::new([]),
        };
        use_case.restore_data.push_back(restore_data.clone());

        assert!(use_case.start(&context).is_ok());

        assert!(matches!(use_case.state, Running(_)), "UseCase isn't running: {:?}", use_case.state);
        let Running(id) = use_case.state else { panic!("UseCase isn't running!") };

        let handle_result = use_case.handle_event(&context, UseCaseEvent::Timer(id));
        assert!(
            handle_result.is_ok(),
            "Handling StoreReq returned error: {:?}",
            handle_result
        );

        let message = use_cases::tests::wait_for_message(broadcast_receiver);
        assert!(matches!(message, ProtocolMessage::StoreReq(_)), "No StoreReq to restore value sent: {:?}", message);
        let ProtocolMessage::StoreReq(ReqRspMessage{data: restored_data, ..})
            = message else { panic!("StoreReq to restore sent") };
        assert_eq!(restore_data.clone(),
                   restored_data,
                   "Restored data isn't equal to original data: {:?} != {:?}",
                    restore_data,
                    restored_data
        )
    }
}
