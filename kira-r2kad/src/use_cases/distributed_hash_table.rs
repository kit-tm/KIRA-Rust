use core::time::Duration;
use derive_more::Display;
use tracing::{Level, instrument};

use std::collections::HashMap;
use std::collections::hash_map::Entry::Occupied;
use std::fmt::Debug;
use std::marker::PhantomData;
use std::num::{NonZeroU8, NonZeroU64, NonZeroUsize};
use std::ops::Deref;

use crate::domain::dht::{DEFAULT_TIMEOUT, RedundancyFactor};
use crate::domain::{
    Age, Contact, ContactState, GroupingError, NodeId, Path, RoutingTable, ULNTable,
    UnderlayNeighborId, dht,
};
use crate::messaging::dht::{
    FetchErr, FetchReqData, FetchRspData, LHTInput, LHTOutput, StoreErr, StoreOk, StoreReqData,
    StoreResult, StoreRspData,
};

use crate::domain::dht::hash_table::{EntryMeta, LocalHashTable};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{
    CommonHeader, ErrorData, FindNodeReqData, Nonce, ProtocolMessage, ProtocolMessageKind,
    ReqRspMessage, WireFormatMessage,
};
use crate::use_cases::{
    ApiEvent, BroadcastableUseCaseEvent, ContactEvent, EventHandler, NeverError, TimerId, UseCase,
    UseCaseContext, UseCaseEvent, UseCaseRuntime, UseCaseState,
};

/// Default interval between garbage collections of the [LocalHashTable].
///
/// To prevent synchronization with other nodes, the actual interval is
/// chosen randomly between 0.5x and 1.5x of this base value.
pub const DEFAULT_COLLECT_INTERVAL: Duration = Duration::from_mins(1);

/// A key-value pair is evicted from the [LocalHashTable] if not accessed for the duration.
pub const DEFAULT_KEY_VALUE_TIMEOUT: Duration = Duration::from_hours(4);

/// TODO: DOCUMENT
pub const DEFAULT_REPUBLISH_SUPPRESSION_WINDOW: Duration = Duration::from_mins(5);

/// Configuration for [DistributedHashTable].
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct DistributedHashTableConfig {
    /// Timeout duration of FindNodeReqs.
    ///
    /// FindNodeReqs are used by StoreReqs to locate
    /// the nearest neighbors for redundancy.
    /// The number of nodes depends on the [redundancy_factor](Self::redundancy_factor).
    pub find_node_timeout: Duration,

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
    /// A key-value pair is accessed by a successful store or fetch request.
    ///
    /// [`collect_interval`]: DistributedHashTableConfig::collect_interval
    pub key_value_timeout: Duration,

    /// Determines the number of additional data replications stored in the network.
    pub redundancy_factor: RedundancyFactor,

    /// TODO: DOCUMENT
    pub republish_suppression_window: Duration,

    pub shared_prefix_bits_grouping: NonZeroU8,
}

impl Default for DistributedHashTableConfig {
    /// Creates a new instance of [DistributedHashTableConfig] with default settings.
    ///
    /// # Example
    ///
    /// ```
    /// use kira_r2kad::use_cases::distributed_hash_table::{
    ///     DistributedHashTableConfig,
    ///     DEFAULT_COLLECT_INTERVAL,
    ///     DEFAULT_REPUBLISH_SUPPRESSION_WINDOW,
    ///     DEFAULT_KEY_VALUE_TIMEOUT
    /// };
    /// use kira_r2kad::domain::dht::DEFAULT_TIMEOUT;
    /// use std::num::NonZeroU8;
    ///
    ///
    /// let config = DistributedHashTableConfig::default();
    ///
    /// assert_eq!(config.find_node_timeout, DEFAULT_TIMEOUT);
    /// assert_eq!(config.collect_interval, DEFAULT_COLLECT_INTERVAL);
    /// assert_eq!(config.key_value_timeout, DEFAULT_KEY_VALUE_TIMEOUT);
    /// assert_eq!(config.republish_suppression_window, DEFAULT_REPUBLISH_SUPPRESSION_WINDOW);
    /// assert_eq!(config.shared_prefix_bits_grouping, NonZeroU8::MIN);
    /// assert_eq!(config.redundancy_factor, RedundancyFactor::default());
    /// ```
    fn default() -> Self {
        Self {
            find_node_timeout: DEFAULT_TIMEOUT,
            collect_interval: DEFAULT_COLLECT_INTERVAL,
            key_value_timeout: DEFAULT_KEY_VALUE_TIMEOUT,
            republish_suppression_window: DEFAULT_REPUBLISH_SUPPRESSION_WINDOW,
            shared_prefix_bits_grouping: NonZeroU8::MIN,
            redundancy_factor: RedundancyFactor::default(),
        }
    }
}

/// Information about the pending response to a request.
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct RequestState {
    response_kind: ProtocolMessageKind,
    response_hook: ResponseHook,
}

#[derive(Debug, Eq, PartialEq, Clone)]
enum ResponseHook {
    RepublishKey { key: NodeId },
}

#[derive(Debug, Display, Eq, PartialEq, Clone)]
pub enum TimerHook {
    #[display("GarbageCollection")]
    GarbageCollection,
    #[display("Timeout: FindNodeReq")]
    TimeoutFindNodeReq(Nonce),
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
    Running {
        timer_hooks: HashMap<TimerId, TimerHook>,
        pending_reqs: HashMap<Nonce, RequestState>,
    },
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
///    its [LocalHashTable] (soft-state).
///    The interval is configurable: [`collect_interval`].
/// 4. **Key Re-Publishing**:
///    Ensures availability under node churn.
///    - _Actively republishes_ key-value pairs to newly joined nodes
///      if the node is key-wise closer to the key-value pair.
///    - _Periodically republishes_ key-value pairs to the k-closest
///      nodes to account for node churn.
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
{
    #[allow(clippy::too_many_arguments)]
    fn register_pending_req(
        &mut self,
        context: &C,
        destination: NodeId,
        expected_nonce: Nonce,
        expected_kind: ProtocolMessageKind,
        timeout: Duration,
        timeout_hook: TimerHook,
        response_hook: ResponseHook,
    ) {
        let DHTState::Running {
            timer_hooks,
            pending_reqs,
            ..
        } = &mut self.state
        else {
            panic!("DistributedHashTable should be running");
        };

        tracing::debug!(
            target: "distributed_hash_table",
            %destination,
            %expected_nonce,
            %expected_kind,
            timeout_ms = timeout.as_millis(),
            "register pending request",
        );

        pending_reqs.insert(
            expected_nonce,
            RequestState {
                response_kind: expected_kind,
                response_hook,
            },
        );

        // set timeout timer
        let timeout_timer_id = context.runtime().register_rand_timer(timeout);
        timer_hooks.insert(timeout_timer_id, timeout_hook);
    }

    fn generate_distinct_nonce(&self) -> Nonce {
        let DHTState::Running { pending_reqs, .. } = &self.state else {
            panic!("DistributedHashTable should be running");
        };

        loop {
            let nonce = Nonce::random();
            if !pending_reqs.contains_key(&nonce) {
                break nonce;
            }
        }
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
    // ========== Sending Message Helper ==========
    fn send_store_rsp(context: &C, store_res: StoreResult, msgid: u64, source_route: SourceRoute) {
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
            not_via: None,
            source_route,
        };

        let protocol_message = ProtocolMessage::from(rsp);
        tracing::trace!(target: "distributed_hash_table", ?protocol_message, "Sending StoreRsp");
        if protocol_message.destination().unwrap() == context.root_id() {
            context
                .runtime()
                .broadcast_event(BroadcastableUseCaseEvent::Message(protocol_message));
            return;
        }

        context
            .runtime()
            .send_message(protocol_message, context.uln_table().deref());
    }

    fn send_dead_end(context: &C, msgid: u64, source_route: SourceRoute) {
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
            not_via: None,
            source_route,
        };

        let protocol_message = ProtocolMessage::from(rsp);
        tracing::trace!(target: "distributed_hash_table", ?protocol_message, "Sending DeadEnd Error");
        if protocol_message.destination().unwrap() == context.root_id() {
            context
                .runtime()
                .broadcast_event(BroadcastableUseCaseEvent::Message(protocol_message));
            return;
        }

        context
            .runtime()
            .send_message(protocol_message, context.uln_table().deref());
    }

    fn send_fetch_rsp(
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
            not_via: None,
            source_route,
        };

        let protocol_message = ProtocolMessage::from(rsp);
        tracing::trace!(target: "distributed_hash_table", ?protocol_message, "Sending FetchRsp");
        if protocol_message.destination().unwrap() == context.root_id() {
            context
                .runtime()
                .broadcast_event(BroadcastableUseCaseEvent::Message(protocol_message));
            return;
        }

        context
            .runtime()
            .send_message(protocol_message, context.uln_table().deref());
    }

    fn send_find_node_req(context: &C, destination: NodeId, k: NonZeroUsize, nonce: Nonce) {
        let neighbors = match k.get().try_into() {
            Ok(value) => value,
            Err(err) => {
                tracing::warn!(
                    target: "distributed_hash_table",
                    %err,
                    "Requested more nodes than FindNodeReq can address. Returning max value.",
                );
                u64::MAX
            }
        };

        let protocol_message: ProtocolMessage = dht::construct_req_rsp_msg_kbr(
            context,
            ProtocolMessageKind::FindNodeReq,
            nonce,
            destination,
            FindNodeReqData {
                exact: false,
                neighborhood: NonZeroU64::new(neighbors).unwrap(),
                target: destination,
            },
        )
        .into();

        tracing::debug!(
            target: "distributed_hash_table",
            %nonce,
            ?protocol_message,
            "Sending FindNodeReq",
        );
        if protocol_message.destination().unwrap() == context.root_id() {
            context
                .runtime()
                .broadcast_event(BroadcastableUseCaseEvent::Message(protocol_message));
            return;
        }

        context
            .runtime()
            .send_message(protocol_message, context.uln_table().deref());
    }

    // ========== Handle Message Responses ==========

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
            let StoreReqData {
                handle: key,
                data: value,
                last_accessed_ms,
            } = message.data;

            let store_res = match self.hash_table.store(key, value) {
                Ok(_) if let Some(last_accessed) = last_accessed_ms => {
                    tracing::debug!(
                        target: "distributed_hash_table",
                        reason = "final destination",
                        "Register republish StoreReq",
                    );

                    let last_accessed = context
                        .runtime()
                        .current_time()
                        .checked_sub(last_accessed.into())
                        .expect("relative last_accessed underflow");

                    self.hash_table
                        .meta_mut(&key)
                        .expect("metadata present for succesfully stored key-value pair")
                        .access(last_accessed);

                    // Republish StoreReq are fire and forget: Don't respond with StoreRsp
                    return;
                }
                Ok(ok) => {
                    self.hash_table
                        .meta_mut(&key)
                        .expect("metadata present for succesfully stored key-value pair")
                        .access(context.runtime().current_time());

                    Ok(ok.into())
                }
                Err(err) => Err(err.into()),
            };

            tracing::debug!(
                target: "distributed_hash_table",
                reason = "final destination",
                ?store_res,
                "Responding to StoreReq with StoreRsp",
            );

            return Self::send_store_rsp(
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

                let path = next_overlay_hop.into_path().unwrap();
                message.source_route.extend(path);
                message.source_route.advance();

                tracing::trace!(
                    target: "distributed_hash_table",
                    reason = "value not stored",
                    next_overlay_hop = %message.destination(),
                    uln = %message
                        .source_route
                        .current_hop(),
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

                Self::send_dead_end(
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
                Self::send_fetch_rsp(
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

            return Self::send_fetch_rsp(
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

                let path = next_overlay_hop.into_path().unwrap();
                message.source_route.extend(path);
                message.source_route.advance();

                tracing::trace!(
                    target: "distributed_hash_table",
                    reason = "value not stored",
                    next_overlay_hop = %message.destination(),
                    uln = %message
                        .source_route
                        .current_hop(),
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
                Self::send_fetch_rsp(
                    context,
                    Err(fetch_err.into()),
                    message.msg_id(),
                    SourceRoute::from_reversed(message.source_route),
                )
            }
        }
    }

    // ========== Republishing ==========
    fn key_republish_needed(&self, context: &C, key: &NodeId) -> bool {
        let Some(meta) = self.hash_table.meta(key) else {
            // keys not stored don't need republishing
            return false;
        };
        let last_republish = meta.last_republish();

        match last_republish {
            Some(last_republish) => {
                let not_republished = context
                    .runtime()
                    .current_time()
                    .saturating_duration_since(last_republish);
                if not_republished > self.config.republish_suppression_window {
                    tracing::debug!(
                        target: "distributed_hash_table",
                        %key,
                        reason = "republish not supressed",
                        republish_supress_window_ms = self.config.republish_suppression_window.as_millis(),
                        not_republished_ms = not_republished.as_millis(),
                        "Republish key",
                    );
                    true
                } else {
                    tracing::trace!(
                        target: "distributed_hash_table",
                        %key,
                        republish_supress_window_ms = self.config.republish_suppression_window.as_millis(),
                        not_republished_ms = not_republished.as_millis(),
                        "Republish key supressed",
                    );
                    false
                }
            }
            None => {
                tracing::debug!(
                    target: "distributed_hash_table",
                    %key,
                    reason = "never republished",
                    "Republish key",
                );
                true
            }
        }
    }

    fn republish_to_contact_if_closer(&mut self, context: &C, contact: Contact) {
        let now = context.runtime().current_time();

        for (key, values) in self.hash_table.fetch_all() {
            let contact_prefix = contact
                .id()
                .shared_prefix_len(key, self.config.shared_prefix_bits_grouping)
                .expect("grouping has to be checked on initialization");
            let root_prefix = context
                .root_id()
                .shared_prefix_len(key, self.config.shared_prefix_bits_grouping)
                .expect("grouping has to be checked on initialization");

            // FIXME: Don't republish to closer node if key-value pair is only kept
            // at the node for redundancy <=> node is not closest to key
            if contact_prefix > root_prefix {
                continue;
            }
            log::debug!(target: "distributed_hash_table", "Replicate local hash entry at nearer node: [{}] at [{}]", key, contact.id());

            let last_accessed_ms = self
                .hash_table
                .meta(key)
                .expect("key in LocalHashTable should have metadata attached")
                .last_access()
                .map(|last_access| {
                    Age::from(now.saturating_duration_since(last_access).as_millis() as u64)
                });

            // TODO: make this more efficient by just sending one big request
            for value in values {
                let data = StoreReqData {
                    handle: *key,
                    data: value,
                    last_accessed_ms,
                };
                dht::send_store_req_kbr(context, Nonce::random(), data, *key);
            }
        }
    }

    fn init_republish_key(&mut self, context: &C, key: NodeId) {
        let destination = key;
        let redundancy = self.config.redundancy_factor.resolve(BUCKET_SIZE);

        let nonce = self.generate_distinct_nonce();
        Self::send_find_node_req(context, destination, redundancy, nonce);

        self.register_pending_req(
            context,
            destination,
            nonce,
            ProtocolMessageKind::FindNodeRsp,
            self.config.find_node_timeout,
            TimerHook::TimeoutFindNodeReq(nonce),
            ResponseHook::RepublishKey { key },
        );
    }

    fn republish_key(
        &self,
        context: &C,
        key: &NodeId,
        source_route_to_closest: SourceRoute,
        kpaths_to_redundant_copies_via_closest: Vec<Path>,
    ) {
        // republish received in the meantime
        if !self.key_republish_needed(context, key) {
            return;
        }
        let Ok(key_data) = self.hash_table.fetch(key) else {
            return;
        };

        let now = context.runtime().current_time();
        let nonce = Nonce::random(); // fire and forget, no distinct Nonce needed
        let last_accessed_ms = self
            .hash_table
            .meta(key)
            .expect("key in LocalHashTable should have metadata attached")
            .last_access()
            .map(|last_access| {
                Age::from(now.saturating_duration_since(last_access).as_millis() as u64)
            });
        let paths: Vec<_> = kpaths_to_redundant_copies_via_closest
            .into_iter()
            .map(|path_via_closest| {
                let mut s = source_route_to_closest.clone();
                s.extend(path_via_closest);
                s
            })
            .collect();

        // TODO: make this more efficient by just sending one big request
        for data in key_data {
            let payload = StoreReqData {
                handle: *key,
                data,
                last_accessed_ms,
            };
            dht::send_store_req(
                context,
                nonce,
                payload.clone(),
                source_route_to_closest.clone(),
            );

            for contact_path in paths.iter().cloned() {
                dht::send_store_req(context, nonce, payload.clone(), contact_path);
            }
        }
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
    fn garbage_collection(&mut self, context: &C) {
        let now = context.runtime().current_time();
        let keys = self
            .hash_table
            .fetch_all()
            .map(|(k, _)| *k)
            .collect::<Vec<_>>();

        for key in keys {
            let meta = self
                .hash_table
                .meta(&key)
                .expect("key in LocalHashTable should have metadata attached");

            // 1. Evict expired key
            let last_access = meta.last_access();
            let expired = match last_access {
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
                        tracing::trace!(
                            target: "distributed_hash_table",
                            %key,
                            reason = "not expired",
                            not_accessed_ms = not_accessed.as_millis(),
                            "Not considering key for eviction",
                        );
                        false
                    }
                }
                None => {
                    tracing::warn!( // warn since this shouldn't happen (store counts as access)
                        target: "distributed_hash_table",
                        %key,
                        reason = "never accessed",
                        "Remove key from local hash table",
                    );
                    true
                }
            };
            if expired {
                assert!(self.hash_table.remove(&key));
                continue;
            }

            // 2. Republish key
            if self.key_republish_needed(context, &key) {
                self.init_republish_key(context, key);
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
        match (event, &mut self.state) {
            // ========== Respond to Requests ==========
            (UseCaseEvent::Message(ProtocolMessage::StoreReq(req), _), _) => {
                self.handle_store_req(context, req);
            }
            (UseCaseEvent::Message(ProtocolMessage::FetchReq(req), _), _) => {
                self.handle_fetch_req(context, req);
            }
            // ========== Handle Timers ==========
            (
                UseCaseEvent::Timer(ref id),
                DHTState::Running {
                    timer_hooks,
                    pending_reqs,
                    ..
                },
            ) => {
                match timer_hooks.remove(id) {
                    Some(timeout_hook @ TimerHook::TimeoutFindNodeReq(nonce)) => {
                        // timeout timer still fires if successfully completed request
                        if let Some(req) = pending_reqs.remove(&nonce) {
                            tracing::debug!(
                                target: "distributed_hash_table",
                                %nonce,
                                kind=%timeout_hook,
                                request=?req,
                                "Request timed out",
                            );
                        };
                    }
                    Some(TimerHook::GarbageCollection) => {
                        // register timer first to use &mut self later
                        let timer_id = context
                            .runtime()
                            .register_rand_timer(self.config.collect_interval);
                        timer_hooks.insert(timer_id, TimerHook::GarbageCollection);

                        self.garbage_collection(context);
                    }
                    None => {}
                };
            }
            // ========== Handle Responses ==========
            (
                UseCaseEvent::Message(message @ ProtocolMessage::FindNodeRsp(_), _),
                DHTState::Running { pending_reqs, .. },
            ) => {
                // FIND CORRESPONDING PENDING REQUEST
                let nonce = message
                    .msg_id()
                    .expect("RPC response message have a message-id");
                let Occupied(req_state_entry) = pending_reqs.entry(nonce) else {
                    return Ok(());
                };
                let expected_response_kind = req_state_entry.get().response_kind;
                if message.kind() != expected_response_kind {
                    tracing::trace!(
                        target: "distributed_hash_table",
                        %nonce,
                        expected_kind = ?expected_response_kind,
                        kind = ?message.kind(),
                        "received unexpected response kind",
                    );
                    return Ok(());
                }

                // HANDLE RESPONSE
                let state = req_state_entry.remove();
                let ResponseHook::RepublishKey { key } = state.response_hook;
                let ProtocolMessage::FindNodeRsp(ReqRspMessage {
                    source_route,
                    data: rtable,
                    ..
                }) = message
                else {
                    unreachable!("only handles FindNodeRsp");
                };
                self.republish_key(
                    context,
                    &key,
                    SourceRoute::from_reversed(source_route),
                    rtable
                        .contacts
                        .into_iter()
                        .filter_map(Contact::into_path)
                        .collect(),
                );
            }
            // ========== Republish values ==========
            (UseCaseEvent::Contact(contact_event), _) => match *contact_event {
                ContactEvent::New(contact) => self.republish_to_contact_if_closer(context, contact),
                ContactEvent::Updated { new, old } if new.is_valid() && !old.is_valid() => {
                    self.republish_to_contact_if_closer(context, *new);
                }
                ContactEvent::Updated { new, old } if new.is_valid() && new.id() != old.id() => {
                    self.republish_to_contact_if_closer(context, *new);
                }
                _ => {}
            },
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

        self.state = DHTState::Running {
            timer_hooks: std::iter::once((timer_id, TimerHook::GarbageCollection)).collect(),
            pending_reqs: Default::default(),
        };

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;
    use std::time::Duration;
    use std::time::Instant;

    use crate::Output;
    use crate::context::ContextConfig;
    use crate::context::SyncContext;
    use crate::domain::dht::hash_table::SingleValueHashTable;
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::domain::underlay::{UnderlayNeighborId, UnderlayNeighborSource};
    use crate::domain::underlay_neighbor_table::in_memory_underlay_neighbor_table::InMemoryULNTable;
    use crate::domain::{Contact, NodeId, Path, SafeStateSeqNr};
    use crate::messaging::dht::{LHTInput, StoreOk, StoreReqData};
    use crate::messaging::messages::{
        CommonHeader, ProtocolMessageKind, ReqRspMessage, WireFormatMessage,
    };
    use crate::messaging::{ProtocolMessage, SourceRoute};
    use crate::runtime::R2KadRuntime;
    use crate::runtime::testing::TestingUseCaseRuntime;
    use crate::use_cases::{EventHandler, UseCase, UseCaseEvent};

    use super::*;

    #[test]
    fn test_store_req_at_destination() {
        crate::tests::init();

        let runtime = TestingUseCaseRuntime::default();
        let root_id = NodeId::with_msb(1);
        let routing_table = SingleBucketRT::<20>::new(root_id);
        let uln_table = InMemoryULNTable::new();

        let sync_context = SyncContext::new(ContextConfig {
            root_id,
            routing_table,
            uln_table,
            insertion_strategy: (),
            runtime,
            vicinity_graph: (),
        });

        let mut dht = DistributedHashTable::<_, SingleValueHashTable, 20>::default();
        dht.start(&sync_context).unwrap();

        let key = NodeId::with_lsb(2);
        let value = LHTInput::from(vec![1, 2, 3]);
        let data = StoreReqData {
            handle: key,
            data: value.clone(),
            last_accessed_ms: None,
        };
        let source_route = SourceRoute::from(root_id);
        let msg = ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::StoreReq,
                root_id,
                root_id,
                Some(123),
                None,
                0,
            ),
            data,
            not_via: None,
            source_route,
        };

        dht.handle_event(
            &sync_context,
            UseCaseEvent::Message(msg.into(), UnderlayNeighborSource::Local),
        )
        .unwrap();

        // Check if value is stored
        let stored_value: Vec<_> = dht.hash_table.fetch(&key).unwrap().collect();
        assert_eq!(stored_value.len(), 1);
        assert_eq!(stored_value[0], value);

        // Check if response is sent
        let output: Vec<_> = sync_context.runtime().broadcast().collect();
        assert_eq!(output.len(), 1);
        if let UseCaseEvent::Message(ProtocolMessage::StoreRsp(rsp), _) = &output[0] {
            assert_eq!(rsp.msg_id(), 123);
            assert_eq!(rsp.data.status, Ok(StoreOk::Created));
        } else {
            panic!("Expected StoreRsp, got {:?}", output[0]);
        }
    }

    #[test]
    fn test_garbage_collection() {
        crate::tests::init();

        let now = Instant::now();
        let runtime = Rc::from(R2KadRuntime::default()); // multiple distinct timers required
        runtime.set_current_time(now);

        let root_id = NodeId::with_lsb(1);
        let routing_table = SingleBucketRT::<20>::new(root_id);
        let uln_table = InMemoryULNTable::new();

        let sync_context = SyncContext::new(ContextConfig {
            root_id,
            routing_table,
            uln_table,
            insertion_strategy: (),
            runtime: runtime.clone(),
            vicinity_graph: (),
        });

        let config = DistributedHashTableConfig {
            key_value_timeout: Duration::from_secs(2),
            collect_interval: Duration::from_secs(1),
            ..DistributedHashTableConfig::default()
        };
        let mut dht = DistributedHashTable::<_, SingleValueHashTable, 20>::new(config).unwrap();
        dht.start(&sync_context).unwrap();

        let key = NodeId::with_lsb(2);
        let value = LHTInput::from(vec![1, 2, 3]);
        dht.hash_table.store(key, value).unwrap();
        dht.hash_table
            .meta_mut(&key)
            .unwrap()
            .access(sync_context.runtime().current_time());

        // value should shouldn't expire instantly
        assert!(dht.hash_table.fetch(&key).is_ok());

        // wait for one second and fire garbage collection timer
        let now = now + Duration::from_secs(1);
        runtime.set_current_time(now);
        while let Some(tid) = runtime.next_timer() {
            dht.handle_event(&sync_context, UseCaseEvent::Timer(tid))
                .unwrap();
        }
        // value should shouldn't expire after one second (but after two)
        assert!(dht.hash_table.fetch(&key).is_ok());

        // key-value pair expires
        println!("EXPIRE HERE >>>");
        let now = now + Duration::from_secs(3);
        runtime.set_current_time(now);

        // fire timer
        // key-value pair should now be evicted
        while let Some(tid) = runtime.next_timer() {
            dht.handle_event(&sync_context, UseCaseEvent::Timer(tid))
                .unwrap();
        }

        // value should be gone
        assert!(dht.hash_table.fetch(&key).is_err());
    }

    #[test]
    fn test_routing_next_hop() {
        crate::tests::init();

        let runtime = TestingUseCaseRuntime::default();
        let root_id = NodeId::with_lsb(0x01);
        let neighbor_id = NodeId::with_lsb(0x30);
        let destination_id = NodeId::with_lsb(0x20);

        let mut routing_table = SingleBucketRT::<20>::new(root_id);
        routing_table
            .insert(Contact::new(
                Path::from(neighbor_id),
                SafeStateSeqNr::try_from(1).unwrap(),
            ))
            .unwrap();

        let mut uln_table = InMemoryULNTable::new();
        uln_table.insert(
            neighbor_id,
            UnderlayNeighborId {
                interface_id: 1.try_into().unwrap(),
                connection_id: 0.into(),
            },
        );

        let sync_context = SyncContext::new(ContextConfig {
            root_id,
            routing_table,
            uln_table,
            insertion_strategy: (),
            runtime,
            vicinity_graph: (),
        });

        let mut dht = DistributedHashTable::<_, SingleValueHashTable, 20>::default();
        dht.start(&sync_context).unwrap();

        let data = StoreReqData {
            handle: destination_id,
            data: LHTInput::from(vec![1, 2, 3]),
            last_accessed_ms: None,
        };
        let msg = ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::StoreReq,
                root_id,
                destination_id,
                Some(123),
                None,
                0,
            ),
            data,
            not_via: None,
            source_route: SourceRoute::from(root_id),
        };

        dht.handle_event(
            &sync_context,
            UseCaseEvent::Message(msg.into(), UnderlayNeighborSource::Local),
        )
        .unwrap();

        // Check if message is forwarded
        let output: Vec<_> = sync_context.runtime().output().collect();
        assert_eq!(output.len(), 1);
        if let Output::SendProtocolMessage(ProtocolMessage::StoreReq(req), _) = &output[0] {
            assert_eq!(req.msg_id(), 123);
            assert_eq!(*req.destination(), neighbor_id);
        } else {
            panic!("Expected forwarded StoreReq, got {:?}", output[0]);
        }
    }

    // TODO: tests for periodic republish
}
