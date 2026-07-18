use core::time::Duration;
use derive_more::Display;

use std::collections::HashMap;
use std::collections::hash_map::Entry::Occupied;
use std::fmt::Debug;
use std::marker::PhantomData;
use std::num::{NonZeroU64, NonZeroUsize};
use std::ops::Deref;
use tracing::{Level, instrument};

use crate::domain::dht::{DEFAULT_TIMEOUT, RedundancyFactor};
use crate::domain::{Contact, NodeId, Path, RoutingTable, ULNTable, UnderlayNeighborId, dht};
use crate::messaging::dht::{FetchReqData, LHTInput, StoreReqData};
use crate::messaging::{
    FindNodeReqData, Nonce, ProtocolMessage, ProtocolMessageKind, ReqRspMessage, SourceRoute,
};
use crate::use_cases::inject_messages::{InjectionResult, errors::InjectMessageError};
use crate::use_cases::{
    BroadcastableUseCaseEvent, EventHandler, FetchInjectData, InjectionMessageData,
    OneshotInjectMessageCallback, StoreInjectData, TimerId, UseCase, UseCaseContext, UseCaseEvent,
    UseCaseRuntime, UseCaseState,
};

/// Default number of seconds between two attempts to restore an existing value.
pub const DEFAULT_PERIODIC_RESTORE: Duration = Duration::from_hours(1);

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

impl Default for DistributedHashTableInjectorConfig {
    /// Returns a new instance of the [DistributedHashTableInjectorConfig]
    /// initialized with the [DEFAULT_PERIODIC_RESTORE].
    ///
    /// # Example
    ///
    /// ```
    /// use kira_r2kad::use_cases::distributed_hash_table_injector::{
    ///     DistributedHashTableInjectorConfig,
    ///     DEFAULT_PERIODIC_RESTORE,
    /// };
    /// use kira_r2kad::domain::dht::DEFAULT_TIMEOUT;
    ///
    ///
    /// let config = DistributedHashTableConfig::default();
    ///
    /// assert_eq!(config.periodic_restore, DEFAULT_PERIODIC_RESTORE);
    /// assert_eq!(config.redundancy_factor, RedundancyFactor::default());
    /// assert_eq!(config.find_node_timeout, DEFAULT_TIMEOUT);
    /// assert_eq!(config.store_timeout, DEFAULT_TIMEOUT);
    /// assert_eq!(config.fetch_timeout, DEFAULT_TIMEOUT);
    /// ```
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
        payload: StoreReqData<LHTInput>,
        restore: bool,
    },
    RegisterStoreRsp {
        payload: StoreReqData<LHTInput>, // for periodic restore
        closest: NodeId,                 // only relay StoreRsp of closest Node
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
    PeriodicRestore(StoreReqData<LHTInput>),
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
///    Periodically restores key-value pairs to keep them stored in the DHT.
///    The duration is configurable: [`periodic_restore`].
/// 2. **Redundancy**:
///    Stores the key-value pairs at the *k* closest nodes.
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
            log::error!(target: "distributed_hash_table", "failed to send inject result: {e:#?}");

            InjectMessageError::SendResultFailed
        })
    }

    fn generate_distinct_nonce(&self) -> Nonce {
        let DHTInjectorState::Running { pending_reqs, .. } = &self.state else {
            panic!("DistributedHashTableInjector should be running");
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

    fn register_periodic_restore(&mut self, context: &C, payload: StoreReqData<LHTInput>) {
        let DHTInjectorState::Running { timer_hooks, .. } = &mut self.state else {
            panic!("DistributedHashTable should be running");
        };

        tracing::trace!(
            target: "distributed_hash_table",
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

    /// **DHT Redundancy**: locate *k* closest nodes to the key to send them store RPCs.
    ///
    /// A node usually doesn't know the *k* closest nodes to the key
    /// because it isn't close to its own [NodeId].
    #[instrument(
        level = Level::DEBUG,
        target = "distributed_hash_table",
        skip(self, context),
    )]
    fn init_redundancy_lookup(
        &mut self,
        context: &C,
        handle: NodeId,
        data: LHTInput,
        restore: bool,
        nonce: Nonce,
    ) -> Result<(), InjectMessageError> {
        let destination = handle;
        let redundancy = self.config.redundancy_factor.resolve(BUCKET_SIZE);

        Self::send_find_node_req(context, destination, redundancy, nonce);

        let payload = StoreReqData {
            handle,
            data,
            last_accessed_ms: None,
        };
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
        target = "distributed_hash_table",
        skip(self, context),
    )]
    fn init_redundant_store(
        &mut self,
        context: &C,
        payload: StoreReqData<LHTInput>,
        source_route_to_closest: SourceRoute,
        paths_to_redundant_copies_via_closest: Vec<Path>,
        nonce: Nonce,
        restore: bool,
    ) {
        let closest = *source_route_to_closest.destination();
        dht::send_store_req(
            context,
            nonce,
            payload.clone(),
            source_route_to_closest.clone(),
        );

        let paths = paths_to_redundant_copies_via_closest
            .into_iter()
            .map(|path_via_closest| {
                // NOTE: StoreReq to redundancy nodes is routed via the key-wise nearest node.
                // It remains to be investigated if whether routing the request
                // via the overlay is favorable.
                let mut s = source_route_to_closest.clone();
                s.extend(path_via_closest);
                s
            });

        // TODO: Spread messages to mitigate incast storm
        for contact_path in paths {
            dht::send_store_req(context, nonce, payload.clone(), contact_path);
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
        target = "distributed_hash_table",
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
        target = "distributed_hash_table",
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
                ProtocolMessage::FindNodeRsp(ReqRspMessage {
                    data: rtable,
                    source_route,
                    ..
                }),
            ) => {
                let redundancy_factor = self.config.redundancy_factor.resolve(BUCKET_SIZE).get();
                if rtable.contacts.len() != redundancy_factor {
                    tracing::warn!(
                        target: "distributed_hash_table",
                        %nonce,
                        redundancy_factor,
                        ?rtable,
                        "Requested redundancy doesn't match size of rtable object",
                    );
                }

                // FIXME: If node with key exists, FindNodeReq(exact=False)
                // returns a hop before. The actual closest node therefor isn't
                // the source of the FindNodeRsp.
                self.init_redundant_store(
                    context,
                    payload,
                    SourceRoute::from_reversed(source_route),
                    rtable
                        .contacts
                        .into_iter()
                        .filter_map(Contact::into_path)
                        .collect(),
                    nonce,
                    restore,
                );

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
                        target: "distributed_hash_table",
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
            _ => unreachable!("Response Hook and Message Kind were previously validated"),
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
                        target: "distributed_hash_table",
                        %nonce,
                        response = ?message,
                        "received unexpected Response",
                    );*/
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

                // CHECK RESPONSE
                let completes_request = match &req_state_entry.get().response_hook {
                    ResponseHook::SendRedundantStoreReqs { .. } => true,
                    ResponseHook::RegisterStoreRsp { closest, .. } => closest == message.source(),
                    ResponseHook::RegisterFetchRsp => true,
                };
                // return early to not mark unhandled responses as complete
                if !completes_request {
                    tracing::trace!(
                        target: "distributed_hash_table",
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
                DHTInjectorState::Running {
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
                                target: "distributed_hash_table",
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
                        let StoreReqData {
                            handle, ref data, ..
                        } = payload;
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
        self.state = DHTInjectorState::Running {
            timer_hooks: HashMap::default(),
            pending_reqs: HashMap::default(),
        };

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    use crate::Output;
    use crate::context::ContextConfig;
    use crate::context::SyncContext;
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::domain::underlay::UnderlayNeighborSource;
    use crate::domain::underlay_neighbor_table::in_memory_underlay_neighbor_table::InMemoryULNTable;
    use crate::domain::{Contact, NodeId, Path, SafeStateSeqNr};
    use crate::messaging::dht::{FetchRspData, LHTInput, StoreOk, StoreRspData};
    use crate::messaging::messages::{
        CommonHeader, ProtocolMessageKind, RTableData, ReqRspMessage, WireFormatMessage,
    };
    use crate::messaging::{ProtocolMessage, SourceRoute};
    use crate::runtime::testing::TestingUseCaseRuntime;
    use crate::use_cases::{EventHandler, UseCase, UseCaseEvent};

    use super::*;

    #[test]
    fn test_store_injection_lifecycle() {
        crate::tests::init();

        let runtime = TestingUseCaseRuntime::default();
        let root_id = NodeId::with_lsb(1);
        let neighbor_id = NodeId::with_lsb(2);
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

        let mut injector = DistributedHashTableInjector::<_, 20>::default();
        injector.start(&sync_context).unwrap();

        let handle = neighbor_id;
        let data = LHTInput::from(vec![1, 2, 3]);
        let (tx, mut rx) = mpsc::unbounded_channel();

        let inject_event = UseCaseEvent::InjectMessage(
            Some(Nonce::from(1)),
            InjectionMessageData::Store(
                StoreInjectData {
                    handle,
                    data: data.clone(),
                    restore: true,
                },
                tx,
            ),
        );

        injector.handle_event(&sync_context, inject_event).unwrap();

        // 1. Should have sent FindNodeReq
        let output: Vec<_> = sync_context.runtime().output().collect();
        assert_eq!(output.len(), 1);
        let nonce =
            if let Output::SendProtocolMessage(ProtocolMessage::FindNodeReq(req), _) = &output[0] {
                assert_eq!(req.data.target, handle);
                req.msg_id()
            } else {
                panic!("Expected FindNodeReq, got {:?}", output[0]);
            };

        // 2. Respond with FindNodeRsp
        let rtable = RTableData { contacts: vec![] };
        let rsp = ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::FindNodeRsp,
                neighbor_id,
                root_id,
                Some(nonce),
                None,
                0,
            ),
            data: rtable,
            not_via: None,
            source_route: SourceRoute::from(Path::try_from(vec![neighbor_id, root_id]).unwrap()),
        };

        injector
            .handle_event(
                &sync_context,
                UseCaseEvent::Message(
                    ProtocolMessage::FindNodeRsp(rsp),
                    UnderlayNeighborSource::Local,
                ),
            )
            .unwrap();

        // 3. Should have sent StoreReq
        let output: Vec<_> = sync_context.runtime().output().collect();
        assert_eq!(output.len(), 1);
        if let Output::SendProtocolMessage(ProtocolMessage::StoreReq(req), _) = &output[0] {
            assert_eq!(req.data.handle, handle);
            assert_eq!(req.data.data, data);
            assert_eq!(*req.destination(), handle);
        } else {
            panic!("Expected StoreReq, got {:?}", output[0]);
        }

        // 4. Respond with StoreRsp
        let store_rsp = ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::StoreRsp,
                handle,
                root_id,
                Some(nonce),
                None,
                0,
            ),
            data: StoreRspData {
                status: Ok(StoreOk::Created),
            },
            not_via: None,
            source_route: SourceRoute::from(handle),
        };

        injector
            .handle_event(
                &sync_context,
                UseCaseEvent::Message(
                    ProtocolMessage::StoreRsp(store_rsp),
                    UnderlayNeighborSource::Local,
                ),
            )
            .unwrap();

        // 5. Callback should have received success
        let result = rx.try_recv().expect("Should have received result");
        if let InjectionResult::Answered(msg) = result {
            assert!(matches!(msg.as_ref(), ProtocolMessage::StoreRsp(_)));
        } else {
            panic!("Expected Answered, got {:?}", result);
        }
    }

    #[test]
    fn test_fetch_injection_lifecycle() {
        crate::tests::init();

        let runtime = TestingUseCaseRuntime::default();
        let root_id = NodeId::with_lsb(1);
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

        let mut injector = DistributedHashTableInjector::<_, 20>::default();
        injector.start(&sync_context).unwrap();

        let handle = NodeId::with_lsb(100);
        let (tx, mut rx) = mpsc::unbounded_channel();

        let inject_event = UseCaseEvent::InjectMessage(
            Some(Nonce::from(2)),
            InjectionMessageData::Fetch(FetchInjectData { handle }, tx),
        );

        injector.handle_event(&sync_context, inject_event).unwrap();

        // 1. Should have sent FetchReq (broadcast because isolated)
        let output: Vec<_> = sync_context.runtime().broadcast().collect();
        assert_eq!(output.len(), 1);
        let nonce = if let UseCaseEvent::Message(ProtocolMessage::FetchReq(req), _) = &output[0] {
            assert_eq!(req.data.handle, handle);
            req.msg_id()
        } else {
            panic!("Expected FetchReq, got {:?}", output[0]);
        };

        // 2. Respond with FetchRsp
        let value = LHTInput::from(vec![1, 2, 3]);
        let fetch_rsp = ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::FetchRsp,
                handle,
                root_id,
                Some(nonce),
                None,
                0,
            ),
            data: FetchRspData {
                data: Ok(vec![value.clone()]),
            },
            not_via: None,
            source_route: SourceRoute::from(handle),
        };

        injector
            .handle_event(
                &sync_context,
                UseCaseEvent::Message(
                    ProtocolMessage::FetchRsp(fetch_rsp),
                    UnderlayNeighborSource::Local,
                ),
            )
            .unwrap();

        // 3. Callback should have received success
        let result = rx.try_recv().expect("Should have received result");
        if let InjectionResult::Answered(msg) = result {
            if let ProtocolMessage::FetchRsp(rsp) = msg.as_ref() {
                let data = rsp.data.data.as_ref().unwrap();
                assert_eq!(data.len(), 1);
                assert_eq!(data[0], value);
            } else {
                panic!("Expected FetchRsp, got {:?}", msg);
            }
        } else {
            panic!("Expected Answered, got {:?}", result);
        }
    }

    #[test]
    fn test_timeout_handling() {
        crate::tests::init();

        let runtime = TestingUseCaseRuntime::default();
        let root_id = NodeId::with_lsb(1);
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

        let mut injector = DistributedHashTableInjector::<_, 20>::default();
        injector.start(&sync_context).unwrap();

        let handle = NodeId::with_lsb(100);
        let (tx, mut rx) = mpsc::unbounded_channel();

        let inject_event = UseCaseEvent::InjectMessage(
            Some(Nonce::from(3)),
            InjectionMessageData::Fetch(FetchInjectData { handle }, tx),
        );

        injector.handle_event(&sync_context, inject_event).unwrap();

        // Fire timeout timer
        let timer_event = sync_context.runtime().timer();
        injector.handle_event(&sync_context, timer_event).unwrap();

        // Callback should have received timeout
        let result = rx.try_recv().expect("Should have received result");
        assert!(matches!(result, InjectionResult::Timeout));
    }
}
