use core::time::Duration;
use derive_more::Display;
use derive_more::From;
use std::collections::HashMap;
use std::collections::hash_map::Entry::Occupied;
use std::fmt::Debug;
use std::marker::PhantomData;
use std::num::{NonZeroU64, NonZeroUsize};
use std::ops::Deref;
use tracing::{Level, instrument};

use crate::domain::{Contact, NodeId, RoutingTable, ULNTable, UnderlayNeighborId, dht};
use crate::use_cases::{
    BroadcastableUseCaseEvent, EventHandler, FetchInjectData, InjectionMessageData,
    OneshotInjectMessageCallback, StoreInjectData, TimerId, UseCase, UseCaseContext, UseCaseEvent,
    UseCaseRuntime, UseCaseState,
};

use crate::messaging::dht::{DefaultLHTInput, FetchReqData, StoreReqData};
use crate::messaging::{
    FindNodeReqData, Nonce, ProtocolMessage, ProtocolMessageKind, ReqRspMessage,
};
use crate::use_cases::distributed_hash_table_injector::DHTInjectorState::Running;
use crate::use_cases::inject_messages::InjectionResult;
use crate::use_cases::inject_messages::errors::InjectMessageError;

/// Default number of seconds between two attempts to restore an existing value.
pub const DEFAULT_PERIODIC_RESTORE: Duration = Duration::from_hours(1);

/// Default timeout duration of the [DistributedHashTableInjector] use case.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// Configuration options for the [DistributedHashTableInjector].
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct DistributedHashTableInjectorConfig {
    /// The duration between periodic restore operations.
    pub periodic_restore: Duration,
    /// Timeout duration of FindNodeReqs.
    ///
    /// FindNodeReqs are used by StoreReqs to locate
    /// the nearest neighbors for redundancy.
    /// The number of nodes depends on the [redundancy_factor](Self::redundancy_factor).
    pub find_node_timeout: Duration,
    /// Timeout duration of StoreReqs
    pub store_timeout: Duration,
    /// Timeout duration of FetchReqs
    pub fetch_timeout: Duration,
    /// Determines the number of additional data replications stored in the network.
    pub redundancy_factor: RedundancyFactor,
}

#[derive(Debug, Default, PartialEq, Eq, From, Clone, Copy)]
/// The redundancy factor ensures that data is stored at multiple locations to prevent loss if nodes
/// go offline.
///
/// The replicas are stored at the *k* key-wise closest nodes of.
/// You can resolve the concrete *k* using [RedundancyFactor::resolve].
///
/// The redundancy factor *includes* the key-wise closest node.
pub enum RedundancyFactor {
    #[default]
    BucketSize,
    #[from]
    Fixed(NonZeroUsize),
}

impl RedundancyFactor {
    /// Resolve the concrete [RedundancyFactor] factor.
    ///
    /// # Examples
    ///
    /// ```
    /// # use kira_r2kad::use_cases::distributed_hash_table_injector::RedundancyFactor;
    ///
    /// let bucket_size = 20;
    /// let fixed = 42;
    ///
    /// // When set to BucketSize it should return the provided bucket size.
    /// let bucket_redundancy = RedundancyFactor::BucketSize;
    /// assert_eq!(bucket_redundancy.resolve(bucket_size).get(), bucket_size);
    /// // The default RedundancyFactor is BucketSize.
    /// assert_eq!(RedundancyFactor::default().resolve(bucket_size).get(), bucket_size);
    ///
    /// // When set to a fixed value it should return the fixed value.
    /// let fixed_redundancy = RedundancyFactor::Fixed(fixed.try_into().unwrap());
    /// assert_eq!(fixed_redundancy.resolve(bucket_size).get(), fixed);
    /// ```
    pub fn resolve(&self, bucket_size: usize) -> NonZeroUsize {
        match self {
            Self::BucketSize => bucket_size.try_into().expect("bucket size > 0"),
            Self::Fixed(redundancy) => *redundancy,
        }
    }
}

impl Default for DistributedHashTableInjectorConfig {
    /// Returns a new instance of the [DistributedHashTableInjectorConfig]
    /// initialized with the [DEFAULT_PERIODIC_RESTORE].
    fn default() -> Self {
        Self {
            periodic_restore: DEFAULT_PERIODIC_RESTORE,
            redundancy_factor: RedundancyFactor::default(),
            find_node_timeout: DEFAULT_TIMEOUT,
            store_timeout: DEFAULT_TIMEOUT,
            fetch_timeout: DEFAULT_TIMEOUT,
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
    SendRedundantStoreReqs {
        payload: StoreReqData<DefaultLHTInput>,
        restore: bool,
    },
    RegisterStoreRsp {
        payload: StoreReqData<DefaultLHTInput>, // for periodic restore
        closest: NodeId,                        // only relay StoreRsp of closest Node
        restore: bool,
    },
    RegisterFetchRsp,
}

#[derive(Debug, Display, Eq, PartialEq, Clone)]
pub enum TimerHook {
    #[display("Timeout: FindNodeReq")]
    TimeoutFindNodeReq(Nonce),
    #[display("Timeout: StoreReq")]
    TimeoutRedundantStoreReq(Nonce),
    #[display("Timeout: FetchReq")]
    TimeoutFetchReq(Nonce),
    #[display("Periodic Restore")]
    PeriodicRestore(StoreReqData<DefaultLHTInput>),
}

/// Represents the state of the [DistributedHashTableInjector] UseCase.
///
/// # Enum Variants
///
/// - `Initialized`: Initial state.
/// - `Running`: State when the UseCase is running.
/// - `Error`: Error state.
#[derive(Debug, PartialEq, Eq, Clone, Default)]
pub enum DHTInjectorState {
    #[default]
    Initialized,
    Running {
        timer_hooks: HashMap<TimerId, TimerHook>,
        pending_reqs: HashMap<Nonce, RequestState>,
    },
    Error,
}

/// The [UseCase] handles incoming [InjectMessage] events related
/// to the *Distributed Hash Table* (DHT).
///
/// The [UseCase] provides the following functionality of the DHT:
///
/// 1. **Periodic Restore**:
///    The [UseCase] periodically restores Key-value pairs.
///    The duration is configurable: [`periodic_restore`].
/// 2. **Redundancy**:
///    The [UseCase] stores the key-value pairs at the *k* closest nodes.
///    The value is configurable: [`redundancy_factor`].
///
/// The [DistributedHashTable] is performing the handling of incoming [StoreReq] and [FetchReq].
///
/// # Generics
///
/// - `C`: [UseCaseContext] in which the UseCase is running in.
///
/// [InjectMessage]: UseCaseEvent::InjectMessage
/// [`periodic_restore`]: DistributedHashTableInjectorConfig::periodic_restore
/// [`redundancy_factor`]: DistributedHashTableInjectorConfig::redundancy_factor
/// [StoreReq]: ProtocolMessage::StoreReq
/// [FetchReq]: ProtocolMessage::FetchReq
/// [DistributedHashTable]: super::distributed_hash_table::DistributedHashTable
#[derive(Debug)]
pub struct DistributedHashTableInjector<C, const BUCKET_SIZE: usize> {
    _c: PhantomData<C>,
    state: DHTInjectorState,
    config: DistributedHashTableInjectorConfig,

    // avoid cloning: store the callbacks centrally.
    callbacks: HashMap<Nonce, OneshotInjectMessageCallback>,
}

impl UseCaseState for DHTInjectorState {
    fn is_error(&self) -> bool {
        matches!(self, Self::Error)
    }
}

impl<C, const BUCKET_SIZE: usize> DistributedHashTableInjector<C, BUCKET_SIZE> {
    /// Creates a new instance of [DistributedHashTableInjector].
    ///
    /// # Arguments
    ///
    /// * `config` - The configuration for the [DistributedHashTableInjector].
    ///
    /// # Returns
    ///
    /// A new instance of [DistributedHashTableInjector].
    pub fn new(config: DistributedHashTableInjectorConfig) -> Self {
        Self {
            _c: PhantomData,
            state: DHTInjectorState::default(),
            config,
            callbacks: HashMap::default(),
        }
    }
}

impl<C, const BUCKET_SIZE: usize> Default for DistributedHashTableInjector<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self::new(DistributedHashTableInjectorConfig::default())
    }
}

impl<C, const BUCKET_SIZE: usize> DistributedHashTableInjector<C, BUCKET_SIZE> {
    fn send_inject_result(
        &self,
        result: InjectionResult,
        callback: OneshotInjectMessageCallback,
    ) -> Result<(), InjectMessageError> {
        callback.send(result).map_err(|e| {
            log::error!(target: "distributed_hash_table_injector", "failed to send inject result: {e:#?}");

            InjectMessageError::SendResultFailed
        })
    }

    fn generate_distinct_nonce(&self) -> Nonce {
        let DHTInjectorState::Running { pending_reqs, .. } = &self.state else {
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

impl<C, const BUCKET_SIZE: usize> DistributedHashTableInjector<C, BUCKET_SIZE>
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
        let DHTInjectorState::Running {
            timer_hooks,
            pending_reqs,
            ..
        } = &mut self.state
        else {
            panic!("DistributedHashTable should be running");
        };

        tracing::debug!(
            target: "distributed_hash_table_injector",
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

    fn register_periodic_restore(&mut self, context: &C, payload: StoreReqData<DefaultLHTInput>) {
        let DHTInjectorState::Running { timer_hooks, .. } = &mut self.state else {
            panic!("DistributedHashTable should be running");
        };

        tracing::trace!(
            target: "distributed_hash_table_injector",
            key = %payload.handle,
            data = ?payload.data.deref(),
            "Registering data for periodic restore",
        );

        let timeout_timer_id = context
            .runtime()
            .register_rand_timer(self.config.periodic_restore);
        timer_hooks.insert(timeout_timer_id, TimerHook::PeriodicRestore(payload));
    }
}

impl<C, const BUCKET_SIZE: usize> DistributedHashTableInjector<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    fn send_find_node_req(
        context: &C,
        destination: NodeId,
        k: NonZeroUsize,
        nonce: Nonce,
    ) -> Result<(), InjectMessageError> {
        let neighbors = match k.get().try_into() {
            Ok(value) => value,
            Err(err) => {
                tracing::warn!(
                    target: "distributed_hash_table_injector",
                    %err,
                    "Requested more nodes than FindNodeReq can address. Returning max value.",
                );
                u64::MAX
            }
        };

        let message: ProtocolMessage = dht::construct_req_rsp_msg(
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
            target: "distributed_hash_table_injector",
            %nonce,
            ?message,
            "Sending FindNodeReq",
        );
        if message.destination().unwrap() == context.root_id() {
            context
                .runtime()
                .broadcast_event(BroadcastableUseCaseEvent::Message(message));
            return Ok(());
        }

        context
            .runtime()
            .send_message(message, context.uln_table().deref());

        Ok(())
    }

    /// **DHT Redundancy**: locate *k* closest nodes to the key to send them store RPCs.
    ///
    /// A node usually doesn't know the *k* closest nodes to the key
    /// because it isn't close to its own [NodeId].
    #[instrument(
        level = Level::DEBUG,
        target = "distributed_hash_table_injector",
        skip(self, context),
    )]
    fn init_redundancy_lookup(
        &mut self,
        context: &C,
        handle: NodeId,
        data: DefaultLHTInput,
        restore: bool,
        nonce: Nonce,
    ) -> Result<(), InjectMessageError> {
        let destination = handle;
        let redundancy = self.config.redundancy_factor.resolve(BUCKET_SIZE);

        Self::send_find_node_req(context, destination, redundancy, nonce)?;

        let payload = StoreReqData { handle, data };
        self.register_pending_req(
            context,
            destination,
            nonce,
            ProtocolMessageKind::FindNodeRsp,
            self.config.find_node_timeout,
            TimerHook::TimeoutFindNodeReq(nonce),
            ResponseHook::SendRedundantStoreReqs { payload, restore },
        );

        Ok(())
    }

    /// **DHT Redundancy**: Store key-value pair (payload) at redundant *k* destinations.
    #[instrument(
        level = Level::DEBUG,
        target = "distributed_hash_table_injector",
        skip(self, context),
    )]
    fn init_redundant_store(
        &mut self,
        context: &C,
        payload: StoreReqData<DefaultLHTInput>,
        destinations: Vec<Contact>,
        nonce: Nonce,
        restore: bool,
    ) {
        let closest = *destinations
            .first()
            .expect("init_redundant_store called with no destinations")
            .id();

        // TODO: Spread messages: mitigate incast storm
        for contact in destinations.into_iter() {
            let destination = *contact.id();
            dht::send_store_req(context, nonce, payload.clone(), destination);
        }

        self.register_pending_req(
            context,
            closest,
            nonce,
            ProtocolMessageKind::StoreRsp,
            self.config.store_timeout,
            TimerHook::TimeoutRedundantStoreReq(nonce),
            ResponseHook::RegisterStoreRsp {
                payload,
                closest,
                restore,
            },
        );
    }

    #[instrument(
        level = Level::DEBUG,
        target = "distributed_hash_table_injector",
        skip(self, context),
    )]
    fn init_fetch_req(
        &mut self,
        context: &C,
        handle: NodeId,
        nonce: Nonce,
    ) -> Result<(), InjectMessageError> {
        let destination = handle;
        dht::send_fetch_req(context, nonce, FetchReqData { handle }, handle)?;

        self.register_pending_req(
            context,
            destination,
            nonce,
            ProtocolMessageKind::FetchRsp,
            self.config.fetch_timeout,
            TimerHook::TimeoutFetchReq(nonce),
            ResponseHook::RegisterFetchRsp,
        );

        Ok(())
    }

    #[instrument(
        level = Level::DEBUG,
        target = "distributed_hash_table_injector",
        skip(self, context),
    )]
    fn handle_response(
        &mut self,
        context: &C,
        response_hook: ResponseHook,
        answered_message: ProtocolMessage,
    ) -> Result<(), InjectMessageError> {
        let nonce = answered_message
            .msg_id()
            .expect("RPC response message have a message-id");

        match (response_hook, answered_message) {
            (
                ResponseHook::SendRedundantStoreReqs { payload, restore },
                ProtocolMessage::FindNodeRsp(ReqRspMessage { data: rtable, .. }),
            ) => {
                let redundancy_factor = self.config.redundancy_factor.resolve(BUCKET_SIZE).get();
                if rtable.contacts.len() != redundancy_factor {
                    tracing::warn!(
                        target: "distributed_hash_table_injector",
                        %nonce,
                        redundancy_factor,
                        ?rtable,
                        "Requested redundancy doesn't match size of rtable object",
                    );
                }

                self.init_redundant_store(context, payload, rtable.contacts, nonce, restore);

                // wait for periodic restore after we confirmed the key-value pair
                // was successfully stored at the closest node
            }
            (
                ResponseHook::RegisterStoreRsp {
                    payload, restore, ..
                },
                answered_message @ ProtocolMessage::StoreRsp(_),
            ) => {
                // no callback on periodic restore
                let Some(callback) = self.callbacks.remove(&nonce) else {
                    return Ok(());
                };
                // relay response
                self.send_inject_result(
                    InjectionResult::Answered(Box::new(answered_message)),
                    callback,
                )?;

                if restore {
                    self.register_periodic_restore(context, payload);
                }
            }
            (ResponseHook::RegisterFetchRsp, answered_message @ ProtocolMessage::FetchRsp(_)) => {
                let Some(callback) = self.callbacks.remove(&nonce) else {
                    tracing::warn!(
                        target: "distributed_hash_table_injector",
                        %nonce,
                        "No callback for FetchReq registered",
                    );
                    return Ok(());
                };

                self.send_inject_result(
                    InjectionResult::Answered(Box::new(answered_message)),
                    callback,
                )?;
            }
            _ => unreachable!("Response Hook and Message Kind where previously validated"),
        }

        Ok(())
    }
}

impl<C, const BUCKET_SIZE: usize> EventHandler for DistributedHashTableInjector<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    type Context = C;
    type Error = InjectMessageError;
    type Value = ();

    #[instrument(
        level = Level::TRACE,
        target = "distributed_hash_table_injector",
        "distributed_hash_table_injector",
        skip(self, context),
        fields(
            state = ?self.state,
            config = ?self.config
        )
    )]
    fn handle_event(
        &mut self,
        context: &Self::Context,
        event: UseCaseEvent,
    ) -> Result<Self::Value, Self::Error> {
        match (event, &mut self.state) {
            // ========== InjectMessage - Store ==========
            (
                UseCaseEvent::InjectMessage(
                    nonce,
                    InjectionMessageData::Store(
                        StoreInjectData {
                            handle,
                            data,
                            restore,
                        },
                        callback,
                    ),
                ),
                DHTInjectorState::Running { .. },
            ) => {
                let nonce = nonce.unwrap_or_else(|| self.generate_distinct_nonce());

                self.callbacks.insert(nonce, callback);
                self.init_redundancy_lookup(context, handle, data, restore, nonce)?;
            }
            // ========== InjectMessage - Fetch ==========
            (
                UseCaseEvent::InjectMessage(
                    nonce,
                    InjectionMessageData::Fetch(FetchInjectData { handle }, callback),
                ),
                _,
            ) => {
                let nonce = nonce.unwrap_or_else(|| self.generate_distinct_nonce());

                self.callbacks.insert(nonce, callback);
                self.init_fetch_req(context, handle, nonce)?;
            }
            // ========== Handle Responses ==========
            (
                UseCaseEvent::Message(
                    // for efficiency and to prohibit nonce colliding
                    message @ (ProtocolMessage::FindNodeRsp(_)
                    | ProtocolMessage::StoreRsp(_)
                    | ProtocolMessage::FetchRsp(_)),
                    _,
                ),
                DHTInjectorState::Running { pending_reqs, .. },
            ) => {
                // FIND CORRESPONDING PENDING REQUEST
                let nonce = message
                    .msg_id()
                    .expect("RPC response message have a message-id");
                let Occupied(req_state_entry) = pending_reqs.entry(nonce) else {
                    /*tracing::trace!(
                        target: "distributed_hash_table_injector",
                        %nonce,
                        response = ?message,
                        "received unexpected Response",
                    );*/
                    return Ok(());
                };
                let expected_response_kind = req_state_entry.get().response_kind;
                if message.kind() != expected_response_kind {
                    tracing::trace!(
                        target: "distributed_hash_table_injector",
                        %nonce,
                        expected_kind = ?expected_response_kind,
                        kind = ?message.kind(),
                        "received unexpected response kind",
                    );
                    return Ok(());
                }

                // CHECK RESPONSE
                let completes_request = match &req_state_entry.get().response_hook {
                    ResponseHook::SendRedundantStoreReqs { .. } => true,
                    ResponseHook::RegisterStoreRsp { closest, .. } => closest == message.source(),
                    ResponseHook::RegisterFetchRsp => true,
                };
                // return early to not mark unhandled responses as complete
                if !completes_request {
                    tracing::trace!(
                        target: "distributed_hash_table_injector",
                        %nonce,
                        response_hook = ?req_state_entry.get().response_hook,
                        "Received message doesn't complete RPC",
                    );
                    return Ok(());
                }

                // HANDLE RESPONSE
                let state = req_state_entry.remove();
                self.handle_response(context, state.response_hook, message)?;
            }
            // ========== Handle Timers ==========
            (
                UseCaseEvent::Timer(ref id),
                Running {
                    timer_hooks,
                    pending_reqs,
                    ..
                },
            ) => {
                match timer_hooks.remove(id) {
                    Some(
                        timeout_hook @ (TimerHook::TimeoutFindNodeReq(nonce)
                        | TimerHook::TimeoutRedundantStoreReq(nonce)
                        | TimerHook::TimeoutFetchReq(nonce)),
                    ) => {
                        // timeout timer still fires if successfully completed request
                        if let Some(req) = pending_reqs.remove(&nonce) {
                            // TODO: log warning if periodic restore timed out

                            tracing::debug!(
                                target: "distributed_hash_table_injector",
                                %nonce,
                                kind=%timeout_hook,
                                request=?req,
                                "Request timed out",
                            );
                            if let Some(callback) = self.callbacks.remove(&nonce) {
                                self.send_inject_result(InjectionResult::Timeout, callback)?;
                            }
                        } else {
                            debug_assert!(
                                self.callbacks.remove(&nonce).is_none(),
                                "orphan callback left over"
                            );
                        }
                    }
                    Some(TimerHook::PeriodicRestore(payload)) => {
                        let StoreReqData { handle, ref data } = payload;
                        let nonce = self.generate_distinct_nonce();
                        self.init_redundancy_lookup(context, handle, data.clone(), false, nonce)?;
                        // schedule restore immediately to restore the value if the request timed out
                        self.register_periodic_restore(context, payload);
                    }
                    None => {}
                };
            }
            _ => {}
        }

        Ok(())
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase for DistributedHashTableInjector<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    type State = DHTInjectorState;

    fn start(&mut self, _: &Self::Context) -> Result<(), Self::Error> {
        self.state = Running {
            timer_hooks: HashMap::default(),
            pending_reqs: HashMap::default(),
        };

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}
