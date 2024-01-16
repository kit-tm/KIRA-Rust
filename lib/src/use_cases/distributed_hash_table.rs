use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use core::time::Duration;
use std::collections::HashSet;
use std::marker::PhantomData;
use std::sync::Arc;

use crate::context::UseCaseContext;
use crate::domain::NodeId;
use crate::messaging::dht::{DefaultLHTInput, DefaultLHTOutput, FetchErr, FetchRspData, StoreResult, StoreRspData};

use crate::domain::dht::TimedValue;
use crate::domain::dht::Expiring;
use crate::domain::dht::hash_table::expiring_hash_table::ExpiringHashTable;
use crate::domain::dht::hash_table::LocalHashTable;
use crate::domain::dht::strategies::fetch_strategy::PermissionlessFetchStrategy;
use crate::domain::dht::strategies::insert_strategy::PermissionlessInsertStrategy;
use crate::domain::dht::strategies::timeout_strategy::ConstTimeoutStrategy;

use crate::messaging::source_route::SourceRoute;
use crate::messaging::{ProtocolMessage, ProtocolMessageSender, ReqRspMessage};
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{ApiEvent, EventHandler, TimerId, UseCase, UseCaseEvent, UseCaseState};
use crate::use_cases::forward_protocol_message::ForwardProtocolMessage;

use crate::domain::dht::strategies::timeout_strategy::DEFAULT_TIMEOUT;

/// Default number of seconds between each garbage collection process.
///
/// This may not be confused with the [DEFAULT_TIMEOUT] used
/// by the [ConstTimeoutStrategy]
/// to determine if a value actually **is** expired.
pub const DEFAULT_COLLECT_INTERVAL: Duration = Duration::from_secs(60);

/// Single data entry in hash table.
pub type HashTableSingle = Arc<[u8]>;
pub type HashTableData = HashSet<TimedValue<HashTableSingle>>;  // Collection of data entries in hash table.

pub type DefaultExpiringHashTable = ExpiringHashTable<
    NodeId,
    HashTableData,
    PermissionlessInsertStrategy,
    PermissionlessFetchStrategy,
    ConstTimeoutStrategy<NodeId, Arc<[u8]>>,
>;

/// Configuration for [DistributedHashTable].
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct DistributedHashTableConfig<H>
{
    /// [LocalHashTable] to store [HashTableSingle] data.
    pub hash_table: H,
    /// The [Duration] between each garbage collection process.
    ///
    /// The garbage collection calls [Expiring::expire] on the [LocalHashTable].
    pub collect_interval: Duration,
}
impl DistributedHashTableConfig<DefaultExpiringHashTable> {
    /// Creates a new instance of [DistributedHashTableConfig] with the given `collect_interval`.
    ///
    /// The `collect_interval` specifies the duration between each garbage collection process
    /// of the internal hash table.
    ///
    /// # Arguments
    ///
    /// - `collect_interval` - The duration between each garbage collection process.
    ///
    /// # Returns
    ///
    /// A new instance of `Self` with the specified `collect_interval`.
    fn with_collect_interval(collect_interval: Duration) -> Self {
        let hash_table = ExpiringHashTable::new(
            PermissionlessInsertStrategy::default(),
            PermissionlessFetchStrategy::default(),
            ConstTimeoutStrategy::default(),
        );

        Self {
            hash_table,
            collect_interval
        }
    }
}

impl Default for DistributedHashTableConfig<DefaultExpiringHashTable> {
    /// Creates a new instance of [DistributedHashTableConfig] with default settings.
    ///
    /// The [DistributedHashTableConfig] returned
    /// is initialized with the [DEFAULT_COLLECT_INTERVAL].
    fn default() -> Self {
        Self::with_collect_interval(DEFAULT_COLLECT_INTERVAL)
    }
}


/// This module defines an enum `DHTError` that represents errors encountered while
/// sending data with DHT protocol.
///
/// # Enum Variants
///
/// - `DHTSendError`: Sending of DHT data over the network failed.
#[derive(Debug)]
pub enum DHTError {
    DHTSendError
}

impl Display for DHTError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "DHTSendError: Sending a response message over the network failed.")
    }
}

impl Error for DHTError {}

/// Represents the state of the [DistributedHashTable] UseCase.
///
/// # Enum Variants
///
/// - `Initialized`: Initial state of the DHT protocol.
/// - `Running(TimerId)`: State when the DHT protocol is running.
///   - `TimerId`: id of the garbage-collection-timer to listen for.
/// - `Error`: Error state in the DHT protocol.
#[derive(Debug, Eq, PartialEq, Clone, Default)]
pub enum DHTState {
    #[default]
    Initialized,
    Running(TimerId),
    Error,
}

/// The [DistributedHashTable] UseCase.
///
/// The UseCase is responsible for handling incoming [StoreReq] and [FetchReq] over the network
/// by sending the respective response.
///
/// For injecting new requests into the network see [DistributedHashTableInjector]
///
/// # Invariants
///
/// This UseCase assumes all DHT requests tasked to handle are addressed to his [LocalHashTable].
/// You need to forward [ProtocolMessage]s over the network yourself if not meant for this Node
///
/// You can use the [ForwardProtocolMessage] UseCase to aid you in this task.
///
/// # Generics
///
/// - `C`: [UseCaseContext] in which the UseCase is running in.
/// - `H`: [LocalHashTable] type used.
pub struct DistributedHashTable<C, H>
{
    _c: PhantomData<C>,
    state: DHTState,
    config: DistributedHashTableConfig<H>,
}

impl<C, H> DistributedHashTable<C, H>
{
    /// Creates a new instance of the [DistributedHashTable].
    ///
    /// # Arguments
    ///
    /// - `config` - The configuration object for the [DistributedHashTable].
    pub fn new(config: DistributedHashTableConfig<H>) -> Self {
        Self {
            _c: PhantomData,
            state: DHTState::default(),
            config,
        }
    }
}

impl<C> Default for DistributedHashTable<C, DefaultExpiringHashTable> {
    /// Constructs a new [DistributedHashTable] instance with the default configuration.
    fn default() -> Self {
        Self::new(DistributedHashTableConfig::default())
    }
}

impl UseCaseState for DHTState {
    fn is_error(&self) -> bool {
        self == &Self::Error
    }
}

impl<C, H, RS> EventHandler for DistributedHashTable<C, H>
    where
        C: UseCaseContext,
        C::MessageSender: ProtocolMessageSender,
        H: LocalHashTable<NodeId, DefaultLHTInput, DefaultLHTOutput, StoreRes=StoreResult, FetchErr=FetchErr> + Expiring<Context=(), Result=RS> + Clone
{
    type Context = C;
    type Error = DHTError;
    type Value = ();

    fn handle_event(&mut self, context: &Self::Context, event: UseCaseEvent) -> Result<Self::Value, Self::Error> {
        match (event, &self.state) {
            (UseCaseEvent::Message(ProtocolMessage::StoreReq(req), _), _) => {
                let res = self.config.hash_table.store(req.data.handle, req.data.data);
                let source_route = SourceRoute::from_reversed(req.source_route);

                let rsp = ReqRspMessage {
                    nonce: req.nonce,
                    source_state_seq_nr: *context.pn_table().state_seq_nr(),
                    data: StoreRspData {
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

                if let Err(e) = context.message_sender_mut().send_message(ProtocolMessage::StoreRsp(rsp)) {
                    log::error!("Failed to send message: {:?}", e);
                    return Err(DHTError::DHTSendError);
                }
            }
            (UseCaseEvent::Message(ProtocolMessage::FetchReq(req), _), _) => {
                let fetch_res = self.config.hash_table.fetch(&req.data.handle);
                let source_route = SourceRoute::from_reversed(req.source_route);

                let rsp = ReqRspMessage {
                    nonce: req.nonce,
                    source_state_seq_nr: *context.pn_table().state_seq_nr(),
                    data: FetchRspData {
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

                if let Err(e) = context.message_sender_mut().send_message(ProtocolMessage::FetchRsp(rsp)) {
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
                let table_dump = self.config.hash_table.fetch_all();

                if let Err(e) = callback.send(table_dump) {
                    log::error!("Failed to send local hash table: {:?}", e);
                    return Err(DHTError::DHTSendError);
                }
            }
            _ => {}
        }

        Ok(())
    }
}


impl<C, H, RS> UseCase for DistributedHashTable<C, H>
    where
        C: UseCaseContext,
        C::MessageSender: ProtocolMessageSender,
        C::Runtime: UseCaseRuntime,
        H: LocalHashTable<NodeId, DefaultLHTInput, DefaultLHTOutput, StoreRes=StoreResult, FetchErr=FetchErr> + Expiring<Context=(), Result=RS> + Clone
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
    use std::time::Instant;
    use crate::broadcaster::MPSCBroadcaster;
    use crate::context::{ContextConfig, SyncContext, UseCaseContext};
    use crate::domain::{InsertionStrategyResult, NetworkInterface, NodeId, Path, PNTable, StateSeqNr, TestInsertionStrategy};
    use crate::domain::dht::hash_table::LocalHashTable;
    use crate::domain::dht::TimedValue;
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::forwarding::in_memory_tables::InMemoryFwdTables;
    use crate::messaging::{AsyncProtocolMessageReceiver, InMemoryMessageChannel, Nonce, ProtocolMessage, ReqRspMessage};
    use crate::messaging::dht::{StoreOK, StoreReqData, StoreRspData};
    use crate::messaging::source_route::SourceRoute;
    use crate::runtime::ImmediateRuntime;
    use crate::use_cases;
    use crate::use_cases::distributed_hash_table::DistributedHashTable;
    use crate::use_cases::{EventHandler, UseCase, UseCaseEvent};

    fn root() -> NodeId { NodeId::with_msb(0) }

    fn sender() -> NodeId { NodeId::with_msb(1) }

    fn data_handle() -> NodeId { NodeId::with_msb(2) }

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
                source_route: SourceRoute::from(Path::from([sender(), root()])),
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

        let ProtocolMessage::StoreRsp(ReqRspMessage { data: StoreRspData { status, .. }, .. })
            = message else { panic!("Wrong response type sent") };
        assert!(matches!(status, Ok(StoreOK::Created)), "Wrong response status returned: {:?}", status);
    }

    #[tokio::test]
    async fn send_storer_on_store_req() {
        crate::tests::init();

        let routing_table = SingleBucketRT::<20>::new(root());
        let interface = NetworkInterface::with_name("test");
        let (hub_sender, mut hub_receiver) = InMemoryMessageChannel::with_interface(interface.clone()).into_parts();
        let (broadcaster, broadcast_receiver) = MPSCBroadcaster::new(10);
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
        assert_eq!(message.destination(), Some(&sender()), "Wrong destination returned: {:?}", message.destination());
    }

    #[tokio::test]
    async fn update_existing_data() {
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
        let existing_data = data();
        let insert_result = use_case.config.hash_table.store(data_handle(), existing_data);
        assert!(insert_result.is_ok(), "Failed to insert data: {:?}", insert_result);
        assert!(use_case.start(&context).is_ok());
        let insert_time = Instant::now();

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

        let ProtocolMessage::StoreRsp(ReqRspMessage { data: StoreRspData { status, .. }, .. })
            = message else { panic!("Wrong response type sent") };
        assert!(matches!(status, Ok(StoreOK::Updated)), "Wrong response status returned: {:?}", status);

        let stored = use_case.config.hash_table.fetch(&data_handle());
        assert!(stored.is_ok(), "Unable to retrieve updated data: {:?}", stored);
        let stored = stored.unwrap();

        assert!(stored.contains(&data()), "Does not contain data to add: {:?}", stored);
        assert_eq!(stored.len(), 1, "Unexpected data length: {} != 1", stored.len());

        let stored_raw = use_case.config.hash_table.map.get(&data_handle())
            .expect("Unable to retrieve raw data");

        let stored_timed = stored_raw.get(&TimedValue::new(data()))
            .expect("Unable to retrieve raw data times");
        assert!(stored_timed.timestamp > insert_time, "Time of value wasn't updated: {:?}", stored_raw);
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

    #[tokio::test]
    async fn append_data_to_existing_data() {
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
        let existing_data: Arc<[u8]> = Arc::new([1, 2, 3, 4]);
        let insert_result = use_case.config.hash_table.store(data_handle(), existing_data.clone());
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

        let ProtocolMessage::StoreRsp(ReqRspMessage { data: StoreRspData { status, .. }, .. })
            = message else { panic!("Wrong response type sent") };
        assert!(matches!(status, Ok(StoreOK::Inserted)), "Wrong response status returned: {:?}", status);

        let stored = use_case.config.hash_table.fetch(&data_handle());
        assert!(stored.is_ok(), "Unable to retrieve updated data: {:?}", stored);
        let stored = stored.unwrap();

        assert_eq!(stored.len(), 2, "Unexpected data length: {} != 2", stored.len());
        assert!(stored.contains(&data()), "Does not contain data to add: {:?}", stored);
        assert!(stored.contains(&existing_data), "Does not contain existing data: {:?}", stored);
    }

    // todo test collect
}
