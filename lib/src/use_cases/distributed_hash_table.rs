use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use core::time::Duration;
use std::collections::HashSet;
use std::marker::PhantomData;
use std::sync::Arc;

use crate::context::UseCaseContext;
use crate::domain::{api, NodeId};
use crate::messaging::dht::{DefaultLHTInput, DefaultLHTOutput, FetchErr, FetchRspData, StoreResult, StoreRspData};

use crate::domain::dht::TimedValue;
use crate::domain::dht::expiring::Expiring;
use crate::domain::dht::hash_table::expiring_hash_table::ExpiringHashTable;
use crate::domain::dht::hash_table::LocalHashTable;
use crate::domain::dht::strategies::fetch_strategy::PermissionlessFetchStrategy;
use crate::domain::dht::strategies::insert_strategy::PermissionlessInsertStrategy;
use crate::domain::dht::strategies::timeout_strategy::ConstTimeoutStrategy;

use crate::messaging::error::SenderError;
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{ProtocolMessage, ProtocolMessageSender, ReqRspMessage};
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{ApiEvent, EventHandler, TimerId, UseCase, UseCaseEvent, UseCaseState};

pub const DEFAULT_COLLECT_INTERVAL: Duration = Duration::from_secs(60);

pub type HashTableSingle = TimedValue<Arc<[u8]>>;
pub type HashTableData = HashSet<HashTableSingle>;
pub type DefaultExpiringHashTable = ExpiringHashTable<
    NodeId,
    HashTableData,
    PermissionlessInsertStrategy,
    PermissionlessFetchStrategy,
    ConstTimeoutStrategy<NodeId, Arc<[u8]>>,
>;


#[derive(Debug, Eq, PartialEq, Clone)]
pub struct DistributedHashTableConfig<H>
{
    hash_table: H,
    collect_interval: Duration,
}

impl Default for DistributedHashTableConfig<DefaultExpiringHashTable> {
    fn default() -> Self {
        let hash_table = ExpiringHashTable::new(
            PermissionlessInsertStrategy::default(),
            PermissionlessFetchStrategy::default(),
            ConstTimeoutStrategy::default(),
        );

        Self {
            hash_table,
            collect_interval: DEFAULT_COLLECT_INTERVAL,
        }
    }
}

#[derive(Debug)]
pub enum DHTError {
    DHTSendError
}

impl Display for DHTError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        todo!()
    }
}

impl Error for DHTError {}

#[derive(Debug, Eq, PartialEq, Clone, Default)]
pub enum DHTState {
    #[default]
    Initialized,
    Running(TimerId),
    Error,
}

pub struct DistributedHashTable<C, H>
{
    _c: PhantomData<C>,
    state: DHTState,
    config: DistributedHashTableConfig<H>,
}

impl<C> Default for DistributedHashTable<C, DefaultExpiringHashTable> {
    fn default() -> Self {
        Self::new(DistributedHashTableConfig::default())
    }
}


impl UseCaseState for DHTState {
    fn is_error(&self) -> bool {
        self == &Self::Error
    }
}

impl<C, H> DistributedHashTable<C, H>
{
    pub fn new(config: DistributedHashTableConfig<H>) -> Self {
        Self {
            _c: PhantomData,
            state: DHTState::default(),
            config,
        }
    }
}

impl<C, H, RS, D> EventHandler for DistributedHashTable<C, H>
    where
        C: UseCaseContext,
        C::Runtime: UseCaseRuntime<SendError=D>,
        D: Debug,
        H: LocalHashTable<NodeId, DefaultLHTInput, DefaultLHTOutput, StoreRes=StoreResult, FetchErr=FetchErr> + Expiring<Context=(), Result=RS> + Clone + Into<api::LocalHashTable>
{
    type Context = C;
    type Error = DHTError;
    type Value = ();

    fn handle_event(&mut self, context: &Self::Context, event: UseCaseEvent) -> Result<Self::Value, Self::Error> {
        match (event, &self.state) {
            (UseCaseEvent::Message(ProtocolMessage::StoreReq(req), _), _) => {
                let res = self.config.hash_table.store(req.data.handle, req.data.data);
                let mut source_route = SourceRoute::from(context.root_id().clone());
                source_route.push_front(context.root_id().clone());

                let rsp = ReqRspMessage {
                    nonce: req.nonce,
                    source_state_seq_nr: *context.pn_table().state_seq_nr(),
                    data: StoreRspData {
                        storer: req.data.storer,
                        status: res
                    },
                    not_via: context.not_via().clone(),
                    source_route,
                };

                log::trace!(
                    target: "dht",
                    "Sending message: {:?}",
                    rsp
                );

                if let Err(e) = context.runtime().send_message(ProtocolMessage::StoreRsp(rsp)) {
                    log::error!("Failed to send message: {:?}", e);
                    return Err(DHTError::DHTSendError);
                }
            }
            (UseCaseEvent::Message(ProtocolMessage::FetchReq(req), _), _) => {
                let fetch_res = self.config.hash_table.fetch(&req.data.handle);
                let mut source_route = SourceRoute::from(context.root_id().clone());
                source_route.push_front(context.root_id().clone());

                let rsp = ReqRspMessage {
                    nonce: req.nonce,
                    source_state_seq_nr: *context.pn_table().state_seq_nr(),
                    data: FetchRspData {
                        fetcher: req.data.fetcher,
                        data: fetch_res,
                    },
                    not_via: context.not_via().clone(),
                    source_route
                };

                log::trace!(
                    target: "dht",
                    "Sending message: {:?}",
                    rsp
                );

                if let Err(e) = context.runtime().send_message(ProtocolMessage::FetchRsp(rsp)) {
                    log::error!("Failed to send message: {:?}", e);
                    return Err(DHTError::DHTSendError);
                }
            }
            (UseCaseEvent::Timer(id), DHTState::Running(our_timer_id)) => {
                if &id == our_timer_id {
                    self.config.hash_table.expire(&());
                }
            }
            // todo move hash table in context and add extra DHTApi UseCase for this event handler
            (UseCaseEvent::API(ApiEvent::LocalHashTable(callback)), _) => {
                if let Err(e) = callback.send(self.config.hash_table.clone().into()) {
                    log::error!("Failed to send local hash table: {:?}", e);
                    return Err(DHTError::DHTSendError);
                }
            }
            _ => {}
        }

        Ok(())
    }
}


impl<C, H, RS, D> UseCase for DistributedHashTable<C, H>
    where
        C: UseCaseContext,
        C::Runtime: UseCaseRuntime<SendError=D>,
        D: Debug,
        H: LocalHashTable<NodeId, DefaultLHTInput, DefaultLHTOutput, StoreRes=StoreResult, FetchErr=FetchErr> + Expiring<Context=(), Result=RS> + Clone + Into<api::LocalHashTable>
{
    type State = DHTState;

    fn start(&mut self, context: &Self::Context) -> Result<(), Self::Error> {
        let timer_id = context
            .runtime()
            .register_periodic_timer(self.config.collect_interval);

        self.state = DHTState::Running(timer_id);

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Arc;
    use crate::broadcaster::MPSCBroadcaster;
    use crate::context::{ContextConfig, SyncContext, UseCaseContext};
    use crate::domain::{InsertionStrategyResult, NetworkInterface, NodeId, PNTable, StateSeqNr, TestInsertionStrategy};
    use crate::domain::dht::hash_table::LocalHashTable;
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::forwarding::in_memory_tables::InMemoryFwdTables;
    use crate::messaging::{AsyncProtocolMessageReceiver, InMemoryMessageChannel, Nonce, ProtocolMessage, ReqRspMessage};
    use crate::messaging::dht::{StoreOK, StoreReqData, StoreResult, StoreRspData};
    use crate::messaging::source_route::SourceRoute;
    use crate::runtime::ImmediateRuntime;
    use crate::use_cases::distributed_hash_table::DistributedHashTable;
    use crate::use_cases::{EventHandler, UseCase, UseCaseEvent};

    fn root() -> NodeId { NodeId::with_msb(0) }

    fn data_handle() -> NodeId { NodeId::with_msb(1) }

    fn data() -> Arc<[u8]> { Arc::new([42]) }

    fn store_req() -> ProtocolMessage {
        ProtocolMessage::StoreReq(
            ReqRspMessage {
                nonce: Nonce::from(1),
                source_state_seq_nr: StateSeqNr::from(0),
                data: StoreReqData {
                    handle: data_handle(),
                    data: data(),
                },
                not_via: Default::default(),
                source_route: SourceRoute::from(root()),
            }
        )
    }

    #[test]
    fn startup_test() {
        let routing_table = SingleBucketRT::<20>::new(root());
        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::with_interface(NetworkInterface::with_name("test")).into_parts();
        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(10);
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let context = SyncContext::new(ContextConfig {
            root_id: root(),
            routing_table,
            pn_table: PNTable::new(),
            insertion_strategy,
            message_sender: hub_sender,
            runtime: ImmediateRuntime::new(broadcaster.clone()),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case = DistributedHashTable::default();
        assert!(use_case.start(&context).is_ok());
    }

    #[test]
    fn handling_store_req() {
        crate::tests::init();

        let routing_table = SingleBucketRT::<20>::new(root());

        let interface = NetworkInterface::with_name("test");
        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::with_interface(interface.clone()).into_parts();

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(10);

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let context = SyncContext::new(ContextConfig {
            root_id: root(),
            routing_table,
            pn_table: PNTable::new(),
            insertion_strategy,
            message_sender: hub_sender,
            runtime: ImmediateRuntime::new(broadcaster.clone()),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case = DistributedHashTable::default();
        assert!(use_case.start(&context).is_ok());

        let receive_event = UseCaseEvent::Message(store_req(), interface.clone());

        let handle_result = use_case.handle_event(&context, receive_event);
        assert!(
            handle_result.is_ok(),
            "Handling a StoreReq returned error: {:?}",
            handle_result
        );
    }

    #[tokio::test]
    async fn send_rsp_on_store_req() {
        crate::tests::init();

        let routing_table = SingleBucketRT::<20>::new(root());
        let interface = NetworkInterface::with_name("test");
        let (hub_sender, mut hub_receiver) = InMemoryMessageChannel::with_interface(interface.clone()).into_parts();
        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(10);
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let context = SyncContext::new(ContextConfig {
            root_id: root(),
            routing_table,
            pn_table: PNTable::new(),
            insertion_strategy,
            message_sender: hub_sender,
            runtime: ImmediateRuntime::new(broadcaster.clone()),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case = DistributedHashTable::default();
        assert!(use_case.start(&context).is_ok());

        let receive_event = UseCaseEvent::Message(store_req(), interface.clone());

        let handle_result = use_case.handle_event(&context, receive_event);
        assert!(
            handle_result.is_ok(),
            "Handling a StoreReq returned error: {:?}",
            handle_result
        );

        let response = hub_receiver.try_recv().await;
        assert!(response.is_ok(), "No result sent: {:?}", response);

        let response = response.unwrap();
        assert!(response.is_some(), "No result sent: {:?}", response);

        let (message, _) = response.unwrap();
        assert!(matches!(message, ProtocolMessage::StoreRsp(_)), "Wrong response type sent: {:?}", message);

        let ProtocolMessage::StoreRsp(ReqRspMessage { data: StoreRspData { status }, .. })
            = message else { panic!("Wrong response type sent") };
        assert!(matches!(status, Ok(StoreOK::Created)), "Wrong response status returned: {:?}", status);
    }

    #[tokio::test]
    async fn send_update_rsp_on_existing_data() {
        crate::tests::init();

        let routing_table = SingleBucketRT::<20>::new(root());
        let interface = NetworkInterface::with_name("test");
        let (hub_sender, mut hub_receiver) = InMemoryMessageChannel::with_interface(interface.clone()).into_parts();
        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(10);
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let context = SyncContext::new(ContextConfig {
            root_id: root(),
            routing_table,
            pn_table: PNTable::new(),
            insertion_strategy,
            message_sender: hub_sender,
            runtime: ImmediateRuntime::new(broadcaster.clone()),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case = DistributedHashTable::default();
        let existing_data = Arc::new([1,2,3,4]);
        let insert_result = use_case.config.hash_table.store(data_handle(), existing_data);
        assert!(insert_result.is_ok(), "Failed to insert data: {:?}", insert_result);
        assert!(use_case.start(&context).is_ok());

        let receive_event = UseCaseEvent::Message(store_req(), interface.clone());

        let handle_result = use_case.handle_event(&context, receive_event);
        assert!(
            handle_result.is_ok(),
            "Handling a StoreReq returned error: {:?}",
            handle_result
        );

        let response = hub_receiver.try_recv().await;
        assert!(response.is_ok(), "No result sent: {:?}", response);

        let response = response.unwrap();
        assert!(response.is_some(), "No result sent: {:?}", response);

        let (message, _) = response.unwrap();
        assert!(matches!(message, ProtocolMessage::StoreRsp(_)), "Wrong response type sent: {:?}", message);

        let ProtocolMessage::StoreRsp(ReqRspMessage { data: StoreRspData { status }, .. })
            = message else { panic!("Wrong response type sent") };
        assert!(matches!(status, Ok(StoreOK::Updated)), "Wrong response status returned: {:?}", status);
    }

    #[test]
    fn store_data_on_store_req() {
        crate::tests::init();

        let routing_table = SingleBucketRT::<20>::new(root());
        let interface = NetworkInterface::with_name("test");
        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::with_interface(interface.clone()).into_parts();
        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(10);
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let context = SyncContext::new(ContextConfig {
            root_id: root(),
            routing_table,
            pn_table: PNTable::new(),
            insertion_strategy,
            message_sender: hub_sender,
            runtime: ImmediateRuntime::new(broadcaster.clone()),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case = DistributedHashTable::default();
        assert!(use_case.start(&context).is_ok());

        let receive_event = UseCaseEvent::Message(store_req(), interface.clone());

        let handle_result = use_case.handle_event(&context, receive_event);
        assert!(
            handle_result.is_ok(),
            "Handling a StoreReq returned error: {:?}",
            handle_result
        );

        let fetch_result = use_case.config.hash_table.fetch(&data_handle());
        assert!(fetch_result.is_ok(), "Failed to insert data: {:?}", fetch_result);

        let stored_data = fetch_result.unwrap();
        assert_eq!(stored_data.len(), 1, "Wrong amount of data stored {:?}", stored_data);

        let stored_data = stored_data.first().unwrap().clone();
        assert_eq!(stored_data, data(), "Data stored is wrong: {:?}", stored_data);
    }

     // todo test collect, appending data to existing data, updating data without inserting new data
}
