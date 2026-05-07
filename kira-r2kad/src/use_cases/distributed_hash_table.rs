use core::time::Duration;
use std::collections::HashMap;
use std::fmt::Debug;
use std::marker::PhantomData;
use std::num::NonZeroU8;
use std::ops::Deref;
use tracing::{Level, instrument};

use crate::domain::{
    Contact, ContactState, GroupingError, NodeId, NotVia, Path, RoutingTable, ULNTable,
    UnderlayNeighborId, dht,
};
use crate::messaging::dht::{
    FetchErr, FetchReqData, FetchRspData, LHTInput, LHTOutput, StoreErr, StoreOk, StoreReqData,
    StoreResult, StoreRspData,
};

use crate::domain::dht::hash_table::{EntryMeta, LocalHashTable};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{
    CommonHeader, ErrorData, Nonce, ProtocolMessage, ProtocolMessageKind, ReqRspMessage,
    WireFormatMessage,
};
use crate::use_cases::{
    ApiEvent, BroadcastableUseCaseEvent, ContactEvent, EventHandler, NeverError, TimerId, UseCase,
    UseCaseContext, UseCaseEvent, UseCaseRuntime, UseCaseState,
};

/// Default interval between garbage collections of the [LocalHashTable].
///
/// To prevent synchronization with other nodes, the actual interval is
/// chosen randomly between 0.5x and 1.5x of this base value.
pub const DEFAULT_COLLECT_INTERVAL: Duration = Duration::from_secs(60);

/// A key-value pair is evicted from the [LocalHashTable] if not accessed for the duration.
pub const DEFAULT_KEY_VALUE_TIMEOUT: Duration = Duration::from_hours(24);

/// Configuration for [DistributedHashTable].
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct DistributedHashTableConfig {
    /// Interval between garbage collections of the [LocalHashTable].
    ///
    /// The garbage collection evicts expired entries from the [LocalHashTable].
    /// To prevent synchronization with other nodes, the actual interval is
    /// chosen randomly between 0.5x and 1.5x of this base value.
    ///
    /// A shorter [Duration] improves the memory footprint by cleaning up
    /// expired entries sooner, but increases CPU overhead.
    /// A longer duration reduces CPU usage but allows expired entries
    /// to persist in memory longer.
    pub collect_interval: Duration,

    /// A key-value pair is considered _expired_ if wasn't accessed for the
    /// entire duration.
    ///
    /// Expired key-value paires get periodically _evicted_ ([`collect_interval`]).
    ///
    /// An key-value pair is accessed by a successful store or fetch request.
    ///
    /// [`collect_interval`]: DistributedHashTableConfig::collect_interval
    pub key_value_timeout: Duration,

    pub shared_prefix_bits_grouping: NonZeroU8,
}

impl Default for DistributedHashTableConfig {
    /// Creates a new instance of [DistributedHashTableConfig] with default settings.
    ///
    /// # Example
    ///
    /// ```
    /// # use kira_r2kad::use_cases::distributed_hash_table::{
    /// #     DistributedHashTableConfig,
    /// #     DEFAULT_COLLECT_INTERVAL,
    /// #     DEFAULT_KEY_VALUE_TIMEOUT
    /// # };
    /// # use std::num::NonZeroU8;
    ///
    /// let config = DistributedHashTableConfig::default();
    ///
    /// assert_eq!(config.collect_interval, DEFAULT_COLLECT_INTERVAL);
    /// assert_eq!(config.key_value_timeout, DEFAULT_KEY_VALUE_TIMEOUT);
    /// assert_eq!(config.shared_prefix_bits_grouping, NonZeroU8::MIN);
    /// ```
    fn default() -> Self {
        Self {
            collect_interval: DEFAULT_COLLECT_INTERVAL,
            key_value_timeout: DEFAULT_KEY_VALUE_TIMEOUT,
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
/// The use case provides the following functionality of the DHT:
///
/// 1. **Key-based Routing (KBR)**:
///    Routes [StoreReq] and [FetchReq] to the key-wise closest node.
///    At each overlay hop the current [SourceRoute] is extended to the key-wise closest node
///    (known to the overlay hop).
/// 2. **Recursive DHT-Value Lookups**:
///    Checks _at each overlay hop_ if the node has stored the key-value pair.
/// 3. **Storage Backend**:
///    Utilizes a [LocalHashTable] to manage and store key-value data.
///    In particular the [UseCase] periodically evicts expired key-value pairs from
///    its [LocalHashTable].
///    The interval is configurable: [`collect_interval`].
/// 4. **Key Re-Publishing**:
///    Ensures availability under node churn.
///    - _Actively republishes_ key-value pairs to newly joined nodes
///      if the node is key-wise closer to the key-value pair.
///    - Periodically _republishes_ key-value pairs to the k-closest
///      nodes to account for node churn. (__not implemented yet__)
///
/// For injecting new requests into the network see the [DistributedHashTableInjector] use case.
///
/// # Generics
///
/// - `C`: [UseCaseContext] in which the UseCase is running in.
/// - `H`: [LocalHashTable] type used.
/// - `BUCKET_SIZE`: Bucket size of the [RoutingTable].
///
/// [StoreReq]: ProtocolMessage::StoreReq
/// [FetchReq]: ProtocolMessage::FetchReq
/// [DistributedHashTableInjector]: super::distributed_hash_table_injector::DistributedHashTableInjector
/// [`collect_interval`]: DistributedHashTableConfig::collect_interval
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
        // TODO: Avoid errors by creating SharedPrefixGrouping newtype that enforces invariant upon creation
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

impl<C, H, const BUCKET_SIZE: usize> DistributedHashTable<C, H, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
    H: LocalHashTable + Clone,
    H::StoreOk: Into<StoreOk>,
    H::StoreErr: Into<StoreErr>,
    H::FetchErr: Into<FetchErr>,
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
        &self,
        context: &C,
        data: Result<LHTOutput, FetchErr>,
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
        for (key, values) in self.hash_table.fetch_all() {
            let contact_prefix = contact
                .id()
                .shared_prefix_len(key, self.config.shared_prefix_bits_grouping)
                .expect("grouping has to be checked on initialization");
            let root_prefix = context
                .root_id()
                .shared_prefix_len(key, self.config.shared_prefix_bits_grouping)
                .expect("grouping has to be checked on initialization");

            if contact_prefix > root_prefix {
                continue;
            }
            // FIXME: Don't republish to closer node if key-value pair is only kept
            // at the node for redundancy <=> node is not closest to key
            log::debug!(target: "distributed_hash_table", "Replicate local hash entry at nearer node: [{}] at [{}]", key, contact.id());

            // TODO: make this more efficient by just sending one big request
            for value in values {
                let data = StoreReqData {
                    handle: *key,
                    data: value,
                };
                dht::send_store_req(context, Nonce::random(), data, *key);
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
        mut message: ReqRspMessage<StoreReqData<LHTInput>>,
    ) {
        let msg_id = message.msg_id();
        let destination = message.dest_id();
        if destination == context.root_id() {
            tracing::debug!(
                target: "distributed_hash_table",
                reason = "final destination",
                "Responding to StoreReq with StoreRsp",
            );
            let key = message.data.handle;
            let value = message.data.data;

            let store_res = match self.hash_table.store(key, value) {
                Ok(ok) => {
                    self.hash_table
                        .meta_mut(&key)
                        .expect("metadata present for succesfully stored key-value pair")
                        .access(context.runtime().current_time());

                    Ok(ok.into())
                }
                Err(err) => Err(err.into()),
            };

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
        let key = &message.data.handle;
        let fetch_err = match self.hash_table.fetch(key).map(|values| values.collect()) {
            Ok(values) => {
                // record access
                self.hash_table
                    .meta_mut(key)
                    .expect("metadata present for succesfully stored key-value pair")
                    .access(context.runtime().current_time());

                tracing::debug!(
                    target: "distributed_hash_table",
                    reason = "value stored",
                    "Responding to FetchReq with FetchRsp",
                );
                self.send_fetch_rsp(
                    context,
                    Ok(values),
                    message.msg_id(),
                    SourceRoute::from_reversed(message.source_route),
                );
                return;
            }
            Err(fetch_err) => fetch_err,
        };

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
                Err(fetch_err.into()),
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
                    Err(fetch_err.into()),
                    message.msg_id(),
                    SourceRoute::from_reversed(message.source_route),
                )
            }
        }
    }
}

impl<C, H, const BUCKET_SIZE: usize> EventHandler for DistributedHashTable<C, H, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
    H: LocalHashTable + Clone,
    H::StoreOk: Into<StoreOk>,
    H::StoreErr: Into<StoreErr>,
    H::FetchErr: Into<FetchErr>,
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
                let now = context.runtime().current_time();
                let keys = self
                    .hash_table
                    .fetch_all()
                    .map(|(k, _)| *k)
                    .collect::<Vec<_>>();

                for key in keys {
                    let last_access = self.hash_table.meta(&key).unwrap().last_access();

                    let remove = match last_access {
                        Some(last_access) => {
                            let not_accessed = now.saturating_duration_since(last_access);
                            if not_accessed > self.config.key_value_timeout {
                                tracing::debug!(
                                    target: "distributed_hash_table",
                                    %key,
                                    reason = "expired",
                                    not_accessed_ms = not_accessed.as_millis(),
                                    "Remove key from local hash table",
                                );
                                true
                            } else {
                                false
                            }
                        }
                        None => {
                            tracing::warn!(
                                target: "distributed_hash_table",
                                %key,
                                reason = "never accessed",
                                "Remove key from local hash table",
                            );
                            true
                        }
                    };

                    if remove {
                        assert!(self.hash_table.remove(&key));
                    }
                }

                let timer_id = context
                    .runtime()
                    .register_rand_timer(self.config.collect_interval);
                self.state = DHTState::Running(timer_id);
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
                let table_dump = self
                    .hash_table
                    .fetch_all()
                    .map(|(k, vs)| (*k, vs.collect()))
                    .collect();

                if let Err(e) = callback.send(table_dump) {
                    log::error!(target: "distributed_hash_table", "Failed to send local hash table: {e:?}");
                }
            }
            _ => {}
        }

        Ok(())
    }
}

impl<C, H, const BUCKET_SIZE: usize> UseCase for DistributedHashTable<C, H, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
    H: LocalHashTable + Clone,
    H::StoreOk: Into<StoreOk>,
    H::StoreErr: Into<StoreErr>,
    H::FetchErr: Into<FetchErr>,
{
    type State = DHTState;

    fn start(&mut self, context: &C) -> Result<(), Self::Error> {
        let timer_id = context
            .runtime()
            .register_rand_timer(self.config.collect_interval);

        self.state = DHTState::Running(timer_id);

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}
