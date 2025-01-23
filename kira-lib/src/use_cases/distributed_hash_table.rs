use core::time::Duration;
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::marker::PhantomData;
use std::ops::Deref;
use std::sync::Arc;

use crate::domain::{
    dht, Contact, ContactState, NodeId, RoutingTable, UNTable, UnderlayNeighborId,
};
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
use crate::messaging::{Nonce, ProtocolMessage, ReqRspMessage};
use crate::use_cases::{
    ApiEvent, BroadcastableUseCaseEvent, ContactEvent, EventHandler, NeverError, TimerId, UseCase,
    UseCaseContext, UseCaseEvent, UseCaseRuntime, UseCaseState,
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

impl<C, H, RS, const BUCKET_SIZE: usize> DistributedHashTable<C, H, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::PhysicalNeighborTable: UNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
    H: LocalHashTable<
            NodeId,
            DefaultLHTInput,
            DefaultLHTOutput,
            StoreRes = StoreResult,
            FetchErr = FetchErr,
        > + Expiring<Context = (), Result = RS>
        + Clone,
{
    fn send_store_rsp(&mut self, context: &C, req: ReqRspMessage<StoreReqData<DefaultLHTInput>>) {
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
            context
                .runtime()
                .broadcast_event(BroadcastableUseCaseEvent::Message(message));
            return;
        }

        context
            .runtime()
            .send_message(message, context.pn_table().deref());
    }

    fn send_fetch_rsp(&mut self, context: &C, req: ReqRspMessage<FetchReqData>) {
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
            context
                .runtime()
                .broadcast_event(BroadcastableUseCaseEvent::Message(message));
            return;
        }

        context
            .runtime()
            .send_message(message, context.pn_table().deref());
    }

    fn republish_to_contact_if_closer(&mut self, context: &C, contact: Contact) {
        for handle in self.hash_table.handles() {
            // TODO support different shared_prefix_len via config
            let contact_prefix = contact
                .id()
                .shared_prefix_len(handle, 1)
                .expect("shared_prefix_len 1 failed");
            let root_prefix = context
                .root_id()
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
                    handle: *handle,
                    data,
                };
                dht::send_store_req(context, Nonce::random(), data);
            }
        }
    }
}

impl<C, H, RS, const BUCKET_SIZE: usize> EventHandler for DistributedHashTable<C, H, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::PhysicalNeighborTable: UNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
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
    type Error = NeverError;
    type Value = ();

    fn handle_event(
        &mut self,
        context: &C,
        event: UseCaseEvent,
    ) -> Result<Self::Value, Self::Error> {
        match (event, &self.state) {
            // ========== Respond to Requests ==========
            (UseCaseEvent::Message(ProtocolMessage::StoreReq(req), _), _) => {
                self.send_store_rsp(context, req)
            }
            (UseCaseEvent::Message(ProtocolMessage::FetchReq(req), _), _) => {
                self.send_fetch_rsp(context, req)
            }
            // ========== Expire Timer event ==========
            (UseCaseEvent::Timer(id), DHTState::Running(our_timer_id)) => {
                if &id == our_timer_id {
                    self.hash_table.expire(&());
                }
            }
            // ========== Republish values ==========
            (UseCaseEvent::Contact(ContactEvent::New(contact)), _) => {
                self.republish_to_contact_if_closer(context, contact)
            }
            (UseCaseEvent::Contact(ContactEvent::Updated { new, old }), _) => {
                if new.state() == &ContactState::Valid && old.state() != &ContactState::Valid {
                    self.republish_to_contact_if_closer(context, new);
                }
            }
            // ========== API Calls ==========
            // TODO move hash table in context and add extra DHTApi UseCase for this event handler
            (UseCaseEvent::API(ApiEvent::LocalHashTable(callback)), _) => {
                let table_dump = self.hash_table.fetch_all();

                if let Err(e) = callback.send(table_dump) {
                    log::error!(target: "distributed_hash_table", "Failed to send local hash table: {:?}", e);
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
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::PhysicalNeighborTable: UNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
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

    fn start(&mut self, context: &C) -> Result<(), Self::Error> {
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
