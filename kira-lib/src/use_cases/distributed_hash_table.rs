use core::time::Duration;
use std::collections::HashSet;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::marker::PhantomData;
use std::sync::Arc;

use crate::context::UseCaseContext;
use crate::domain::{dht, Contact, ContactState, NetworkInterface, NodeId, PNTable, RoutingTable};
use crate::messaging::dht::{
    DefaultLHTInput, DefaultLHTOutput, FetchErr, FetchReqData, FetchRspData, StoreReqData,
    StoreResult, StoreRspData,
};

use crate::domain::dht::hash_table::expiring_hash_table::ExpiringHashTable;
use crate::domain::dht::hash_table::LocalHashTable;
use crate::domain::dht::strategies::fetch_strategy::PermissionlessFetchStrategy;
use crate::domain::dht::strategies::insert_strategy::PermissionlessInsertStrategy;
use crate::domain::dht::strategies::timeout_strategy::ConstTimeoutStrategy;
use crate::domain::dht::Expiring;
use crate::domain::dht::TimedValue;

use crate::messaging::source_route::SourceRoute;
use crate::messaging::{Nonce, ProtocolMessage, ProtocolMessageSender, ReqRspMessage};
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{
    ApiEvent, ContactEvent, EventHandler, TimerId, UseCase, UseCaseEvent, UseCaseState,
};

/// Default number of seconds between each garbage collection process.
///
/// This may not be confused with the [DEFAULT_TIMEOUT](crate::domain::dht::strategies::timeout_strategy::DEFAULT_TIMEOUT) used
/// by the [ConstTimeoutStrategy]
/// to determine if a value actually **is** expired.
pub const DEFAULT_COLLECT_INTERVAL: Duration = Duration::from_secs(60);

/// Single data entry in hash table.
pub type HashTableSingle = Arc<[u8]>;

/// Data kept internally in the [ExpiringHashTable]
///
/// This is a collection of multiple [HashTableSingle]s tagged with a
/// creation timestamp to determine if they have expired. (see [TimedValue])
pub type HashTableData = HashSet<TimedValue<HashTableSingle>>;

/// This is the [ExpiringHashTable] with all the default strategies.
pub type DefaultExpiringHashTable = ExpiringHashTable<
    NodeId,
    HashTableData,
    PermissionlessInsertStrategy,
    PermissionlessFetchStrategy,
    ConstTimeoutStrategy<NodeId, Arc<[u8]>>,
>;

/// Configuration for [DistributedHashTable].
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct DistributedHashTableConfig {
    /// The [Duration] between each garbage collection process.
    ///
    /// The garbage collection calls [Expiring::expire] on the [LocalHashTable].
    pub collect_interval: Duration,
}
impl DistributedHashTableConfig {
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
        Self { collect_interval }
    }
}

impl Default for DistributedHashTableConfig {
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
    SendError,
}

impl Display for DHTError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "DHTSendError: Sending a response message over the network failed."
        )
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
/// The UseCase is responsible for handling incoming [StoreReq](ProtocolMessage::StoreReq) and [FetchReq](ProtocolMessage::FetchReq) over the network
/// by sending the respective response.
///
/// For injecting new requests into the network see the
/// [DistributedHashTableInjector](super::distributed_hash_table_injector::DistributedHashTableInjector) UseCase
///
/// # Invariants
///
/// This UseCase assumes all DHT requests tasked to handle are addressed to his [LocalHashTable].
/// You need to forward [ProtocolMessage]s over the network yourself if not meant for this Node
///
/// You can use the [ForwardProtocolMessage](crate::use_cases::forward_protocol_message::ForwardProtocolMessage) UseCase to aid you in this task.
///
/// # Generics
///
/// - `C`: [UseCaseContext] in which the UseCase is running in.
/// - `H`: [LocalHashTable] type used.
/// - `BUCKET_SIZE`: Bucket size of the [RoutingTable].
pub struct DistributedHashTable<C, H, const BUCKET_SIZE: usize> {
    _c: PhantomData<C>,
    state: DHTState,
    config: DistributedHashTableConfig,
    hash_table: H,
}

impl<C, H: Default, const BUCKET_SIZE: usize> DistributedHashTable<C, H, BUCKET_SIZE> {
    /// Creates a new instance of the [DistributedHashTable].
    ///
    /// # Arguments
    ///
    /// - `config` - The configuration object for the [DistributedHashTable].
    pub fn new(config: DistributedHashTableConfig) -> Self {
        Self {
            _c: PhantomData,
            state: DHTState::default(),
            config,
            hash_table: H::default(),
        }
    }
}

impl<C, H: Default, const BUCKET_SIZE: usize> Default for DistributedHashTable<C, H, BUCKET_SIZE> {
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

impl<C, H, RS, D: Debug, const BUCKET_SIZE: usize> DistributedHashTable<C, H, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::MessageSender: ProtocolMessageSender,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::Runtime: UseCaseRuntime<SendError = D>,
    C::PhysicalNeighborTable: PNTable,
    H: LocalHashTable<
            NodeId,
            DefaultLHTInput,
            DefaultLHTOutput,
            StoreRes = StoreResult,
            FetchErr = FetchErr,
        > + Expiring<Context = (), Result = RS>
        + Clone,
{
    fn send_store_rsp(
        &mut self,
        context: &C,
        req: ReqRspMessage<StoreReqData<DefaultLHTInput>>,
    ) -> Result<(), DHTError> {
        let res = self.hash_table.store(req.data.handle, req.data.data);
        let source_route = SourceRoute::from_reversed(req.source_route);

        let rsp = ReqRspMessage {
            nonce: req.nonce,
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
            data: StoreRspData { status: res },
            not_via: context.not_via().clone(),
            source_route,
        };

        log::trace!(target: "distributed_hash_table", "Sending message: {:?}", rsp);

        let message = ProtocolMessage::StoreRsp(rsp);
        if message.destination().unwrap() == context.root_id() {
            return context.runtime().broadcast_local_event(UseCaseEvent::Message(message, NetworkInterface::loopback())).map_err(|e| {
                log::error!(target: "distributed_hash_table", "Failed to send message: {:?}", e);
                DHTError::SendError
            });
        }
        if let Err(e) = context.message_sender_mut().send_message(message) {
            log::error!(target: "distributed_hash_table", "Failed to send message: {:?}", e);
            return Err(DHTError::SendError);
        }

        Ok(())
    }

    fn send_fetch_rsp(
        &mut self,
        context: &C,
        req: ReqRspMessage<FetchReqData>,
    ) -> Result<(), DHTError> {
        let fetch_res = self.hash_table.fetch(&req.data.handle);
        let source_route = SourceRoute::from_reversed(req.source_route);

        let rsp = ReqRspMessage {
            nonce: req.nonce,
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
            data: FetchRspData { data: fetch_res },
            not_via: context.not_via().clone(),
            source_route,
        };

        log::trace!(target: "distributed_hash_table", "Sending message: {:?}", rsp);

        let message = ProtocolMessage::FetchRsp(rsp);
        if message.destination().unwrap() == context.root_id() {
            return context.runtime().broadcast_local_event(UseCaseEvent::Message(message, NetworkInterface::loopback())).map_err(|e| {
                log::error!(target: "distributed_hash_table", "Failed to send message: {:?}", e);
                DHTError::SendError
            });
        }

        if let Err(e) = context.message_sender_mut().send_message(message) {
            log::error!(target: "distributed_hash_table", "Failed to send message: {:?}", e);
            return Err(DHTError::SendError);
        }

        Ok(())
    }

    fn republish_to_contact_if_closer(&mut self, context: &C, contact: Contact) -> Result<(), DHTError> {
        let root_id = context.root_id();
        for handle in self.hash_table.into_handles() {
            // TODO support different shared_prefix_len via config
            let contact_prefix = contact
                .id()
                .shared_prefix_len(handle, 1)
                .expect("shared_prefix_len 1 failed");
            let root_prefix = root_id
                .shared_prefix_len(handle, 1)
                .expect("shared_prefix_len 1 failed");

            if contact_prefix > root_prefix {
                continue;
            }
            log::debug!(target: "distributed_hash_table", "Replicate local hash entry at nearer node: [{}] at [{}]", handle, contact.id());

            let entry = self
                .hash_table
                .peek(handle)
                .expect("Fetching existing handle failed");

            // TODO make this more efficient by just sending one big request
            // TODO make it configurable to delete the value after a successful store
            for data in entry {
                let data = StoreReqData {
                    handle: handle.clone(),
                    data,
                };
                if let Err(e) = dht::send_store_req(context, Nonce::random(), data) {
                    log::error!(target: "distributed_hash_table", "Failed to replicate local hash entry at nearer node: {:?}", e);
                    return Err(DHTError::SendError);
                }
            }
        }

        Ok(())
    }
}

impl<C, H, RS, D: Debug, const BUCKET_SIZE: usize> EventHandler
    for DistributedHashTable<C, H, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::MessageSender: ProtocolMessageSender,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::Runtime: UseCaseRuntime<SendError = D>,
    C::PhysicalNeighborTable: PNTable,
    H: LocalHashTable<
            NodeId,
            DefaultLHTInput,
            DefaultLHTOutput,
            StoreRes = StoreResult,
            FetchErr = FetchErr,
        > + Expiring<Context = (), Result = RS>
        + Clone,
{
    type Context = C;
    type Error = DHTError;
    type Value = ();

    fn handle_event(
        &mut self,
        context: &Self::Context,
        event: UseCaseEvent,
    ) -> Result<Self::Value, Self::Error> {
        match (event, &self.state) {
            // ========== Respond to Requests ==========
            (UseCaseEvent::Message(ProtocolMessage::StoreReq(req), _), _) => {
                self.send_store_rsp(context, req)?
            }
            (UseCaseEvent::Message(ProtocolMessage::FetchReq(req), _), _) => {
                self.send_fetch_rsp(context, req)?
            }
            // ========== Expire Timer event ==========
            (UseCaseEvent::Timer(id), DHTState::Running(our_timer_id)) => {
                if &id == our_timer_id {
                    self.hash_table.expire(&());
                }
            }
            // ========== Republish values ==========
            (UseCaseEvent::Contact(ContactEvent::New(contact)), _) => self.republish_to_contact_if_closer(context, contact)?, 
            (UseCaseEvent::Contact(ContactEvent::Updated { new, old }), _) => {
                if new.state() == &ContactState::Valid && old.state() != &ContactState::Valid {
                    self.republish_to_contact_if_closer(context, new)?;
                }
            }
            // ========== API Calls ==========
            // TODO move hash table in context and add extra DHTApi UseCase for this event handler
            (UseCaseEvent::API(ApiEvent::LocalHashTable(callback)), _) => {
                let table_dump = self.hash_table.fetch_all();

                if let Err(e) = callback.send(table_dump) {
                    log::error!(target: "distributed_hash_table", "Failed to send local hash table: {:?}", e);
                    return Err(DHTError::SendError);
                }
            }
            _ => {}
        }

        Ok(())
    }
}

impl<C, H, RS, const BUCKET_SIZE: usize> UseCase for DistributedHashTable<C, H, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::MessageSender: ProtocolMessageSender,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::Runtime: UseCaseRuntime,
    C::PhysicalNeighborTable: PNTable,
    H: LocalHashTable<
            NodeId,
            DefaultLHTInput,
            DefaultLHTOutput,
            StoreRes = StoreResult,
            FetchErr = FetchErr,
        > + Expiring<Context = (), Result = RS>
        + Clone,
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
    use crate::broadcaster::MPSCBroadcaster;
    use crate::context::{ContextConfig, SyncContext, UseCaseContext};
    use crate::domain::dht::hash_table::LocalHashTable;
    use crate::domain::dht::TimedValue;
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::domain::{
        InMemoryPNTable, InsertionStrategyResult, NetworkInterface, NodeId, Path, StateSeqNr,
        TestInsertionStrategy,
    };
    use crate::forwarding::in_memory_tables::InMemoryFwdTables;
    use crate::messaging::dht::{StoreOK, StoreReqData, StoreRspData};
    use crate::messaging::source_route::SourceRoute;
    use crate::messaging::{
        AsyncProtocolMessageReceiver, InMemoryMessageChannel, Nonce, ProtocolMessage, ReqRspMessage,
    };
    use crate::runtime::ImmediateRuntime;
    use crate::use_cases::distributed_hash_table::{
        DefaultExpiringHashTable, DistributedHashTable,
    };
    use crate::use_cases::{EventHandler, UseCase, UseCaseEvent};
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::time::Instant;

    fn root() -> NodeId {
        NodeId::with_msb(0)
    }

    fn sender() -> NodeId {
        NodeId::with_msb(1)
    }

    fn data_handle() -> NodeId {
        NodeId::with_msb(2)
    }

    fn data() -> Arc<[u8]> {
        Arc::new([42])
    }

    fn store_req() -> ProtocolMessage {
        ProtocolMessage::StoreReq(ReqRspMessage {
            nonce: Nonce::from(1),
            source_state_seq_nr: StateSeqNr::from(0),
            data: StoreReqData {
                handle: data_handle(),
                data: data(),
            },
            not_via: Default::default(),
            source_route: SourceRoute::from(Path::from([sender(), root()])),
        })
    }

    #[test]
    fn startup_test() {
        let routing_table = SingleBucketRT::<20>::new(root());
        let (hub_sender, _hub_receiver) =
            InMemoryMessageChannel::with_interface(NetworkInterface::with_name("test"))
                .into_parts();
        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(10);
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let context = SyncContext::new(ContextConfig {
            root_id: root(),
            routing_table,
            pn_table: InMemoryPNTable::new(),
            insertion_strategy,
            message_sender: hub_sender,
            runtime: ImmediateRuntime::new(broadcaster.clone()),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case: DistributedHashTable<_, DefaultExpiringHashTable> =
            DistributedHashTable::default();
        assert!(use_case.start(&context).is_ok());
    }

    #[test]
    fn handling_store_req() {
        crate::tests::init();

        let routing_table = SingleBucketRT::<20>::new(root());

        let interface = NetworkInterface::with_name("test");
        let (hub_sender, _hub_receiver) =
            InMemoryMessageChannel::with_interface(interface.clone()).into_parts();

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(10);

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let context = SyncContext::new(ContextConfig {
            root_id: root(),
            routing_table,
            pn_table: InMemoryPNTable::new(),
            insertion_strategy,
            message_sender: hub_sender,
            runtime: ImmediateRuntime::new(broadcaster.clone()),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case: DistributedHashTable<_, DefaultExpiringHashTable> =
            DistributedHashTable::default();
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
        let (hub_sender, mut hub_receiver) =
            InMemoryMessageChannel::with_interface(interface.clone()).into_parts();
        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(10);
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let context = SyncContext::new(ContextConfig {
            root_id: root(),
            routing_table,
            pn_table: InMemoryPNTable::new(),
            insertion_strategy,
            message_sender: hub_sender,
            runtime: ImmediateRuntime::new(broadcaster.clone()),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case: DistributedHashTable<_, DefaultExpiringHashTable> =
            DistributedHashTable::default();
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
        assert!(
            matches!(message, ProtocolMessage::StoreRsp(_)),
            "Wrong response type sent: {:?}",
            message
        );

        let ProtocolMessage::StoreRsp(ReqRspMessage {
            data: StoreRspData { status, .. },
            ..
        }) = message
        else {
            panic!("Wrong response type sent")
        };
        assert!(
            matches!(status, Ok(StoreOK::Created)),
            "Wrong response status returned: {:?}",
            status
        );
    }

    #[tokio::test]
    async fn send_storer_on_store_req() {
        crate::tests::init();

        let routing_table = SingleBucketRT::<20>::new(root());
        let interface = NetworkInterface::with_name("test");
        let (hub_sender, mut hub_receiver) =
            InMemoryMessageChannel::with_interface(interface.clone()).into_parts();
        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(10);
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let context = SyncContext::new(ContextConfig {
            root_id: root(),
            routing_table,
            pn_table: InMemoryPNTable::new(),
            insertion_strategy,
            message_sender: hub_sender,
            runtime: ImmediateRuntime::new(broadcaster.clone()),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case: DistributedHashTable<_, DefaultExpiringHashTable> =
            DistributedHashTable::default();
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
        assert_eq!(
            message.destination(),
            Some(&sender()),
            "Wrong destination returned: {:?}",
            message.destination()
        );
    }

    #[tokio::test]
    async fn update_existing_data() {
        crate::tests::init();

        let routing_table = SingleBucketRT::<20>::new(root());
        let interface = NetworkInterface::with_name("test");
        let (hub_sender, mut hub_receiver) =
            InMemoryMessageChannel::with_interface(interface.clone()).into_parts();
        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(10);
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let context = SyncContext::new(ContextConfig {
            root_id: root(),
            routing_table,
            pn_table: InMemoryPNTable::new(),
            insertion_strategy,
            message_sender: hub_sender,
            runtime: ImmediateRuntime::new(broadcaster.clone()),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case: DistributedHashTable<_, DefaultExpiringHashTable> =
            DistributedHashTable::default();
        let existing_data = data();
        let insert_result = use_case.hash_table.store(data_handle(), existing_data);
        assert!(
            insert_result.is_ok(),
            "Failed to insert data: {:?}",
            insert_result
        );
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
        assert!(
            matches!(message, ProtocolMessage::StoreRsp(_)),
            "Wrong response type sent: {:?}",
            message
        );

        let ProtocolMessage::StoreRsp(ReqRspMessage {
            data: StoreRspData { status, .. },
            ..
        }) = message
        else {
            panic!("Wrong response type sent")
        };
        assert!(
            matches!(status, Ok(StoreOK::Updated)),
            "Wrong response status returned: {:?}",
            status
        );

        let stored = use_case.hash_table.fetch(&data_handle());
        assert!(
            stored.is_ok(),
            "Unable to retrieve updated data: {:?}",
            stored
        );
        let stored = stored.unwrap();

        assert!(
            stored.contains(&data()),
            "Does not contain data to add: {:?}",
            stored
        );
        assert_eq!(
            stored.len(),
            1,
            "Unexpected data length: {} != 1",
            stored.len()
        );

        let stored_raw = use_case
            .hash_table
            .map
            .get(&data_handle())
            .expect("Unable to retrieve raw data");

        let stored_timed = stored_raw
            .get(&TimedValue::new(data()))
            .expect("Unable to retrieve raw data times");
        assert!(
            stored_timed.timestamp > insert_time,
            "Time of value wasn't updated: {:?}",
            stored_raw
        );
    }

    #[test]
    fn store_data_on_store_req() {
        crate::tests::init();

        let routing_table = SingleBucketRT::<20>::new(root());
        let interface = NetworkInterface::with_name("test");
        let (hub_sender, _hub_receiver) =
            InMemoryMessageChannel::with_interface(interface.clone()).into_parts();
        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(10);
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let context = SyncContext::new(ContextConfig {
            root_id: root(),
            routing_table,
            pn_table: InMemoryPNTable::new(),
            insertion_strategy,
            message_sender: hub_sender,
            runtime: ImmediateRuntime::new(broadcaster.clone()),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case: DistributedHashTable<_, DefaultExpiringHashTable> =
            DistributedHashTable::default();
        assert!(use_case.start(&context).is_ok());

        let receive_event = UseCaseEvent::Message(store_req(), interface.clone());

        let handle_result = use_case.handle_event(&context, receive_event);
        assert!(
            handle_result.is_ok(),
            "Handling a StoreReq returned error: {:?}",
            handle_result
        );

        let fetch_result = use_case.hash_table.fetch(&data_handle());
        assert!(
            fetch_result.is_ok(),
            "Failed to insert data: {:?}",
            fetch_result
        );

        let stored_data = fetch_result.unwrap();
        assert_eq!(
            stored_data.len(),
            1,
            "Wrong amount of data stored {:?}",
            stored_data
        );

        let stored_data = stored_data.first().unwrap().clone();
        assert_eq!(
            stored_data,
            data(),
            "Data stored is wrong: {:?}",
            stored_data
        );
    }

    #[tokio::test]
    async fn append_data_to_existing_data() {
        crate::tests::init();

        let routing_table = SingleBucketRT::<20>::new(root());
        let interface = NetworkInterface::with_name("test");
        let (hub_sender, mut hub_receiver) =
            InMemoryMessageChannel::with_interface(interface.clone()).into_parts();
        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(10);
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let context = SyncContext::new(ContextConfig {
            root_id: root(),
            routing_table,
            pn_table: InMemoryPNTable::new(),
            insertion_strategy,
            message_sender: hub_sender,
            runtime: ImmediateRuntime::new(broadcaster.clone()),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case: DistributedHashTable<_, DefaultExpiringHashTable> =
            DistributedHashTable::default();
        let existing_data: Arc<[u8]> = Arc::new([1, 2, 3, 4]);
        let insert_result = use_case
            .hash_table
            .store(data_handle(), existing_data.clone());
        assert!(
            insert_result.is_ok(),
            "Failed to insert data: {:?}",
            insert_result
        );
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
        assert!(
            matches!(message, ProtocolMessage::StoreRsp(_)),
            "Wrong response type sent: {:?}",
            message
        );

        let ProtocolMessage::StoreRsp(ReqRspMessage {
            data: StoreRspData { status, .. },
            ..
        }) = message
        else {
            panic!("Wrong response type sent")
        };
        assert!(
            matches!(status, Ok(StoreOK::Inserted)),
            "Wrong response status returned: {:?}",
            status
        );

        let stored = use_case.hash_table.fetch(&data_handle());
        assert!(
            stored.is_ok(),
            "Unable to retrieve updated data: {:?}",
            stored
        );
        let stored = stored.unwrap();

        assert_eq!(
            stored.len(),
            2,
            "Unexpected data length: {} != 2",
            stored.len()
        );
        assert!(
            stored.contains(&data()),
            "Does not contain data to add: {:?}",
            stored
        );
        assert!(
            stored.contains(&existing_data),
            "Does not contain existing data: {:?}",
            stored
        );
    }

    // todo test collect
}
