use core::time::Duration;
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::marker::PhantomData;
use std::num::NonZeroU8;
use std::ops::Deref;
use std::sync::Arc;
use tracing::{Level, instrument};

use crate::domain::{
    Contact, ContactState, GroupingError, NodeId, NotVia, Path, RoutingTable, ULNTable,
    UnderlayNeighborId, dht,
};
use crate::messaging::dht::{
    DefaultLHTInput, DefaultLHTOutput, FetchErr, FetchReqData, FetchRspData, StoreReqData,
    StoreResult, StoreRspData,
};

use crate::domain::dht::hash_table::{LocalHashTable, expiring_hash_table::ExpiringHashTable};
use crate::domain::dht::strategies::{
    fetch_strategy::PermissionlessFetchStrategy, insert_strategy::PermissionlessInsertStrategy,
    timeout_strategy::ConstTimeoutStrategy,
};
use crate::domain::dht::{Expiring, TimedValue};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{
    CommonHeader, ErrorData, Nonce, ProtocolMessage, ProtocolMessageKind, ReqRspMessage,
    WireFormatMessage,
};
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

    pub shared_prefix_bits_grouping: NonZeroU8,
}

impl Default for DistributedHashTableConfig {
    /// Creates a new instance of [DistributedHashTableConfig] with default settings.
    ///
    /// # Example
    ///
    /// ```
    /// # use kira_r2kad::use_cases::distributed_hash_table::{DistributedHashTableConfig, DEFAULT_COLLECT_INTERVAL};
    /// # use std::num::NonZeroU8;
    ///
    /// let config = DistributedHashTableConfig::default();
    ///
    /// assert_eq!(config.collect_interval, DEFAULT_COLLECT_INTERVAL);
    /// assert_eq!(config.shared_prefix_bits_grouping, NonZeroU8::MIN);
    /// ```
    fn default() -> Self {
        Self {
            collect_interval: DEFAULT_COLLECT_INTERVAL,
            shared_prefix_bits_grouping: NonZeroU8::MIN,
        }
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
#[derive(Debug)]
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
    pub fn new(config: DistributedHashTableConfig) -> Result<Self, GroupingError> {
        // TODO: Avoid errors by creating SharedPrefixGrouping newtype that inforces
        // invariant upon creation
        if config.shared_prefix_bits_grouping.get() > NodeId::BITS {
            return Err(GroupingError::Invalid {
                group_size: config.shared_prefix_bits_grouping,
            });
        };

        Ok(Self {
            _c: PhantomData,
            state: DHTState::default(),
            config,
            hash_table: H::default(),
        })
    }
}

impl<C, H: Default, const BUCKET_SIZE: usize> Default for DistributedHashTable<C, H, BUCKET_SIZE> {
    /// Constructs a new [DistributedHashTable] instance with the default configuration.
    fn default() -> Self {
        Self::new(DistributedHashTableConfig::default())
            .expect("Valid grouping with default config")
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
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
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
        store_res: StoreResult,
        msgid: u64,
        source_route: SourceRoute,
    ) {
        let rsp = ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::StoreRsp,
                *context.root_id(),
                *source_route.destination(),
                Some(msgid),
                Some(From::from(*context.uln_table().state_seq_nr())),
                context.uln_table().size(),
            ),
            data: StoreRspData { status: store_res },
            not_via: context.not_via_state().iter().map(NotVia::from).collect(),
            source_route,
        };

        let message = ProtocolMessage::from(rsp);
        tracing::trace!(target: "distributed_hash_table", ?message, "Sending StoreRsp");
        if message.destination().unwrap() == context.root_id() {
            context
                .runtime()
                .broadcast_event(BroadcastableUseCaseEvent::Message(message));
            return;
        }

        context
            .runtime()
            .send_message(message, context.uln_table().deref());
    }

    fn send_dead_end(&mut self, context: &C, msgid: u64, source_route: SourceRoute) {
        let rsp = ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::Error,
                *context.root_id(),
                *source_route.destination(),
                Some(msgid),
                Some(From::from(*context.uln_table().state_seq_nr())),
                context.uln_table().size(),
            ),
            data: ErrorData::DeadEnd,
            not_via: context.not_via_state().iter().map(NotVia::from).collect(),
            source_route,
        };

        let message = ProtocolMessage::from(rsp);
        tracing::trace!(target: "distributed_hash_table", ?message, "Sending DeadEnd Error");
        if message.destination().unwrap() == context.root_id() {
            context
                .runtime()
                .broadcast_event(BroadcastableUseCaseEvent::Message(message));
            return;
        }

        context
            .runtime()
            .send_message(message, context.uln_table().deref());
    }

    fn send_fetch_rsp(
        &mut self,
        context: &C,
        data: Result<DefaultLHTOutput, FetchErr>,
        msgid: u64,
        source_route: SourceRoute,
    ) {
        let rsp = ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::FetchRsp,
                *context.root_id(),
                *source_route.destination(),
                Some(msgid),
                Some(From::from(*context.uln_table().state_seq_nr())),
                context.uln_table().size(),
            ),
            data: FetchRspData { data },
            not_via: context.not_via_state().iter().map(NotVia::from).collect(),
            source_route,
        };

        let message = ProtocolMessage::from(rsp);
        tracing::trace!(target: "distributed_hash_table", ?message, "Sending FetchRsp");
        if message.destination().unwrap() == context.root_id() {
            context
                .runtime()
                .broadcast_event(BroadcastableUseCaseEvent::Message(message));
            return;
        }

        context
            .runtime()
            .send_message(message, context.uln_table().deref());
    }

    fn republish_to_contact_if_closer(&mut self, context: &C, contact: Contact) {
        for handle in self.hash_table.handles() {
            let contact_prefix = contact
                .id()
                .shared_prefix_len(handle, self.config.shared_prefix_bits_grouping)
                .expect("grouping has to be checked on initialization");
            let root_prefix = context
                .root_id()
                .shared_prefix_len(handle, self.config.shared_prefix_bits_grouping)
                .expect("grouping has to be checked on initialization");

            if contact_prefix > root_prefix {
                continue;
            }
            log::debug!(target: "distributed_hash_table", "Replicate local hash entry at nearer node: [{}] at [{}]", handle, contact.id());

            let entry = self
                .hash_table
                .peek(handle)
                .expect("Fetching existing handle failed");

            // TODO: make this more efficient by just sending one big request
            for data in entry {
                let data = StoreReqData {
                    handle: *handle,
                    data,
                };
                dht::send_store_req(context, Nonce::random(), data, *handle);
            }
        }
    }

    #[instrument(
        level = Level::DEBUG,
        target = "distributed_hash_table",
        "distributed_hash_table",
        skip_all,
        fields(
            key = %message.data.handle,
            destination = %message.dest_id(),
        )
    )]
    fn handle_store_req(
        &mut self,
        context: &C,
        mut message: ReqRspMessage<StoreReqData<DefaultLHTInput>>,
    ) {
        let msg_id = message.msg_id();
        let destination = message.dest_id();
        if destination == context.root_id() {
            tracing::debug!(
                target: "distributed_hash_table",
                reason = "final destination",
                "Responding to FetchReq with FetchRsp",
            );

            let store_res = self
                .hash_table
                .store(message.data.handle, message.data.data);

            return self.send_store_rsp(
                context,
                store_res,
                msg_id,
                SourceRoute::from_reversed(message.source_route),
            );
        }

        // route to next overlay hop
        match context
            .routing_table()
            .next_hop(destination, self.config.shared_prefix_bits_grouping)
            .expect("grouping has to be checked on initialization")
        {
            Some(next_overlay_hop) => {
                assert_eq!(
                    next_overlay_hop.state(),
                    &ContactState::Valid,
                    "invalid next overlay overlay hop"
                );

                let path: Path = next_overlay_hop.into();
                message.source_route.extend(path);
                message.source_route.advance();

                tracing::trace!(
                    target: "distributed_hash_table",
                    reason = "value not stored",
                    next_overlay_hop = %message.destination(),
                    uln = %message
                        .source_route
                        .next_hop()
                        .expect("should have next hop after extending to next overlay hop"),
                    "Route StoreReq to next overlay hop by key-based routing",
                );

                context
                    .runtime()
                    .send_message(message, context.uln_table().deref())
            }
            None => {
                tracing::debug!(
                    target: "distributed_hash_table",
                    reason = "unable to complete route to destination",
                    "Responding to StoreReq with DeadEnd Error",
                );

                self.send_dead_end(
                    context,
                    message.msg_id(),
                    SourceRoute::from_reversed(message.source_route),
                )
            }
        }
    }

    #[instrument(
        level = Level::DEBUG,
        target = "distributed_hash_table",
        "distributed_hash_table",
        skip_all,
        fields(
            key = %message.data.handle,
            destination = %message.dest_id(),
        )
    )]
    fn handle_fetch_req(&mut self, context: &C, mut message: ReqRspMessage<FetchReqData>) {
        let fetch_res = self.hash_table.fetch(&message.data.handle);
        if fetch_res.is_ok() {
            tracing::debug!(
                target: "distributed_hash_table",
                reason = "value stored",
                "Responding to FetchReq with FetchRsp",
            );

            return self.send_fetch_rsp(
                context,
                fetch_res,
                message.msg_id(),
                SourceRoute::from_reversed(message.source_route),
            );
        }

        let destination = message.dest_id();
        if destination != &message.data.handle {
            // the message is going to routed by destination
            // therefor the resulting routing will not be key-based routing
            tracing::warn!(
                target: "distributed_hash_table",
                key = %message.data.handle,
                destination = %message.dest_id(),
                "Received FetchReq where destination doesn't match requested key",
            );
        }
        if destination == context.root_id() {
            tracing::debug!(
                target: "distributed_hash_table",
                reason = "final destination",
                "Responding to FetchReq with FetchRsp",
            );

            return self.send_fetch_rsp(
                context,
                fetch_res,
                message.msg_id(),
                SourceRoute::from_reversed(message.source_route),
            );
        }

        // route to next overlay hop (ideally this results in key-based routing)
        match context
            .routing_table()
            .next_hop(destination, self.config.shared_prefix_bits_grouping)
            .expect("grouping has to be checked on initialization")
        {
            Some(next_overlay_hop) => {
                assert_eq!(
                    next_overlay_hop.state(),
                    &ContactState::Valid,
                    "invalid next overlay overlay hop"
                );

                let path: Path = next_overlay_hop.into();
                message.source_route.extend(path);
                message.source_route.advance();

                tracing::trace!(
                    target: "distributed_hash_table",
                    reason = "value not stored",
                    next_overlay_hop = %message.destination(),
                    uln = %message
                        .source_route
                        .next_hop()
                        .expect("should have next hop after extending to next overlay hop"),
                    "Route FetchReq to next overlay hop",
                );

                context
                    .runtime()
                    .send_message(message, context.uln_table().deref())
            }
            None => {
                tracing::debug!(
                    target: "distributed_hash_table",
                    reason = "unable to route to nearer overlay hop",
                    "Responding to FetchReq with FetchRsp",
                );
                self.send_fetch_rsp(
                    context,
                    fetch_res,
                    message.msg_id(),
                    SourceRoute::from_reversed(message.source_route),
                )
            }
        }
    }
}

impl<C, H, RS, const BUCKET_SIZE: usize> EventHandler for DistributedHashTable<C, H, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
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

    #[instrument(
        level = Level::TRACE,
        target = "distributed_hash_table",
        "distributed_hash_table",
        skip(self, context),
        fields(
            state = ?self.state,
            config = ?self.config
        )
    )]
    fn handle_event(
        &mut self,
        context: &C,
        event: UseCaseEvent,
    ) -> Result<Self::Value, Self::Error> {
        match (event, &self.state) {
            // ========== Respond to Requests ==========
            (UseCaseEvent::Message(ProtocolMessage::StoreReq(req), _), _) => {
                self.handle_store_req(context, req);
            }
            (UseCaseEvent::Message(ProtocolMessage::FetchReq(req), _), _) => {
                self.handle_fetch_req(context, req);
            }
            // ========== Expire Timer event ==========
            (UseCaseEvent::Timer(id), DHTState::Running(our_timer_id)) if &id == our_timer_id => {
                // checks the whole hash table for expired values

                // TODO: log expired values
                self.hash_table.expire(&());
            }
            // ========== Republish values ==========
            (UseCaseEvent::Contact(ContactEvent::New(contact)), _) => {
                self.republish_to_contact_if_closer(context, contact)
            }
            (UseCaseEvent::Contact(ContactEvent::Updated { new, old }), _)
                if new.state() == &ContactState::Valid && old.state() != &ContactState::Valid =>
            {
                self.republish_to_contact_if_closer(context, new);
            }
            (UseCaseEvent::Contact(ContactEvent::Updated { new, old }), _)
                if new.state() == &ContactState::Valid && new.id() != old.id() =>
            {
                self.republish_to_contact_if_closer(context, new);
            }
            // ========== API Calls ==========
            (UseCaseEvent::API(ApiEvent::LocalHashTable(callback)), _) => {
                let table_dump = self.hash_table.fetch_all();

                if let Err(e) = callback.send(table_dump) {
                    log::error!(target: "distributed_hash_table", "Failed to send local hash table: {e:?}");
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
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
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
