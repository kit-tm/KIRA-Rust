use std::cmp::min;
use std::collections::{HashMap, hash_map};
use std::fmt::Debug;
use std::marker::PhantomData;
use std::num::NonZeroUsize;
use std::ops::Deref;
use std::time::Duration;
use tracing::{Level, field, instrument};

use derive_more::derive::{Display, Error};

use crate::domain::UnderlayNeighborDestination::{Broadcast, Multicast, UnderlayNeighbor};
use crate::domain::VICINITY_RADIUS;
use crate::domain::{
    Contact, DEFAULT_BUCKET_SIZE, InterfaceId, NodeId, Path, RoutingTable, SafeStateSeqNr,
    StateSeqNr, ULNTable, UnderlayNeighborId, UnderlayNeighborSource, UnderlayNeighborUpdate,
    VicinityGraph, node_id,
};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{
    HelloMessage, Nonce, ProtocolMessage, QueryRouteReqData, QueryRouteType, RTableData,
    ReqRspMessage,
};
use crate::use_cases::{
    ApiEvent, EventHandler, TimerId, UseCase, UseCaseContext, UseCaseEvent, UseCaseRuntime,
    UseCaseState, VicinityEvent,
};

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub struct VicinityDiscoveryConfig {
    /// Initial interval to use for sending ULNHello messages.
    ///
    /// The interval between subsequent ULNHello messages is doubled every time.
    /// to a maximum of [`uln_max_interval`](Self::uln_max_interval).
    pub uln_min_interval: Duration,
    /// Maximum interval to use for sending ULNHello messages.
    pub uln_max_interval: Duration,
    /// Initial maximum wait-time to receive the dedicated ULNDiscRsp.
    ///
    /// The wait-time is doubled between consecutive tries until
    /// [`uln_discovery_max_retries`](Self::uln_discovery_max_retries) is reached.
    pub uln_discovery_rsp_initial_max_wait_time: Duration,
    /// Maximum retries to complete an ULNDiscovery handshake with an underlay neighbor.
    pub uln_discovery_max_retries: usize,

    /// Number of bits for the deterministic heuristic to consider for deciding which node should
    /// respond to the ULNHello message.
    pub heuristic_calculation_bits: NonZeroUsize,
    /// Maximum delay of synchronisation acceptable
    /// caused by not responding to an ULNHello because of the heuristic.
    pub heuristic_max_wait_time: Duration,

    /// Timeout duration to use for processing the queue of nodes that need to be resynced.
    pub resync_timeout: Duration,
    /// Maximum number of nodes that are resynced in parallel during the periodic
    /// resync phase.
    ///
    /// Urgent resyncs can be queried immediately and temporarily lead to more nodes queried.
    pub max_parallel_resync_count: usize,
}

impl Default for VicinityDiscoveryConfig {
    fn default() -> Self {
        Self {
            uln_min_interval: Duration::from_millis(200), // 100 ms wireless
            uln_max_interval: Duration::from_secs(30),    // 1 s wireless
            uln_discovery_rsp_initial_max_wait_time: Duration::from_millis(200),
            uln_discovery_max_retries: 2,

            heuristic_calculation_bits: NonZeroUsize::new(32).unwrap(),
            heuristic_max_wait_time: Duration::from_secs(1),
            resync_timeout: Duration::from_millis(100),
            max_parallel_resync_count: 10, // TODO: determine sensible default
        }
    }
}

/// Error types for vicinity discovery.
#[derive(Debug, Display, Error)]
pub enum VDError {
    /// A Contact contains an invalid neighbor.
    #[display("Contact contains invalid neighbor")]
    NeighborInconsistency,
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct InterfaceState {
    hello_interval: Duration,
}

#[derive(Debug, Eq, PartialEq, Clone)]
/// Information about the pending response to a request.
pub struct RequestState {
    timeouts: usize,
    timeout: Duration,
    expected_nonce: Nonce,
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum TimerHook {
    /// Repeat a request to the node.
    ///
    /// A repeat is initialized after a timeout timer fired.
    RepeatReq(NodeId),
    /// Multicast a ULNHello on the interface.
    SendInterfaceHello(InterfaceId),
}

#[derive(Debug, Eq, PartialEq, Clone, Default)]
pub enum VDState {
    #[default]
    Initialized,
    Running {
        resync_timer_id: TimerId,
        timer_hooks: HashMap<TimerId, TimerHook>,

        pending_reqs: HashMap<NodeId, RequestState>,
        interfaces: HashMap<InterfaceId, InterfaceState>,
    },
    Error,
}

impl UseCaseState for VDState {
    fn is_error(&self) -> bool {
        self == &Self::Error
    }
}

/// The vicinity discovery (VD) use case.
///
/// Handles the discovery of the underlay neighborhood (*vicinity*) including exchanging messages
/// with underlay neighbors.
///
/// If a new [Contact] was added to the [RoutingTable] or an existing one was updated and
/// has a underlay distance in the range of [1, [VICINITY_RADIUS]] hops (*path length is in
/// [2, [VICINITY_RADIUS] + 1]) a QueryRouteReq is sent to them to get their underlay neighbors.
#[derive(Debug)]
pub struct VicinityDiscovery<C, const BUCKET_SIZE: usize = DEFAULT_BUCKET_SIZE> {
    _c: PhantomData<C>,
    state: VDState,
    config: VicinityDiscoveryConfig,
}

impl<C, const BUCKET_SIZE: usize> Default for VicinityDiscovery<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self::new(VicinityDiscoveryConfig::default())
    }
}

/// Returns if the node should answer to the other nodes ULNHello.
fn deterministic_heuristic(self_id: &NodeId, other: &NodeId, num_bits: NonZeroUsize) -> bool {
    // Use deterministic heuristic to determine if we should respond to the Message
    // do not use full ID, otherwise large IDs will always "loose", use mod 2^{calculation_bits} comparison
    // small collision chance: but just in case, full nodeID will be a tie breaker

    // Unwrapping is safe here, as checked at construction
    let own_bits = self_id.bits(0, num_bits).unwrap();
    let other_bits = other.bits(0, num_bits).unwrap();
    let delta = other_bits.wrapping_sub(own_bits);

    // Simulation: (delta < 0x80000000) || ((delta == 0 || delta == 0x80000000) && context.root_id() < &source)
    if delta < 0x80000000 && delta != 0 {
        return true;
    }
    if (delta == 0 || delta == 0x80000000) && self_id < other {
        return true;
    }

    false
}

fn requires_sync(node: &NodeId, vicinity_graph: &impl VicinityGraph) -> bool {
    debug_assert!(
        vicinity_graph.observed_ssn(node) >= vicinity_graph.vicinity_ssn(node),
        "observed ssn >= vicinity ssn"
    );

    vicinity_graph.vicinity_ssn(node) < vicinity_graph.observed_ssn(node)
}

impl<C, const BUCKET_SIZE: usize> VicinityDiscovery<C, BUCKET_SIZE> {
    /// Create a new vicinity discovery use case in [VDState::Initialized].
    pub fn new(config: VicinityDiscoveryConfig) -> Self {
        if node_id::BIT_SIZE < config.heuristic_calculation_bits.get() {
            panic!(
                "Number of bits to use for the heuristic in VicinityDiscovery is greater than BIT_SIZE of NodeId."
            )
        }
        Self {
            _c: PhantomData,
            state: VDState::default(),
            config,
        }
    }
}

impl<C, const BUCKET_SIZE: usize> VicinityDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
{
    fn set_next_resync_timeout(&mut self, context: &C) {
        // only update if we are running already
        if let VDState::Running {
            resync_timer_id, ..
        } = &mut self.state
        {
            *resync_timer_id = context
                .runtime()
                .register_rand_timer(self.config.resync_timeout);
        }
    }
}

// message crafting and sending
impl<C, const BUCKET_SIZE: usize> VicinityDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
    C::VicinityGraph: VicinityGraph,
{
    fn send_query_route_req(context: &C, path: Path, nonce: Nonce) -> Result<(), VDError> {
        // Crucially Nodes _on_ the Vicinity Radius are also excluded
        // because we discover them using their neighbors inside the Vicinity
        assert!(
            path.size() < VICINITY_RADIUS,
            "shouldn't send QueryRouteReq outside the vicinity"
        );
        assert!(
            path.size() > 1,
            "shouldn't send QueryRouteReq to underlay neighbors"
        );

        // Convert contacts path to source route
        let mut source_route = SourceRoute::from(path.clone());
        source_route.push_front(*context.root_id());

        // Get interface of route
        let Some(underlay_neighbor) = context.uln_table().get(path.first()).cloned() else {
            tracing::error!(
                target: "vicinity_discovery",
                ?path,
                reason = "neighbor_inconsistency",
                "abort sending QueryRouteReq"
            );
            return Err(VDError::NeighborInconsistency);
        };

        // Request only underlay Neighborhood of that Node
        let request = ReqRspMessage {
            nonce,
            source_state_seq_nr: From::from(*context.uln_table().state_seq_nr()),
            data: QueryRouteReqData {
                query_type: QueryRouteType::UnderlayNeighbors,
            },
            not_via: context.not_via().clone(),
            source_route,
        };

        tracing::debug!(target: "vicinity_discovery", ?request, "send QueryRouteReq");
        context
            .runtime()
            .send_message_via(request, underlay_neighbor.into());
        Ok(())
    }

    fn send_query_route_rsp(
        &self,
        context: &C,
        request: ReqRspMessage<QueryRouteReqData>,
        underlay_destination: UnderlayNeighborId,
    ) -> Result<(), VDError> {
        let contacts = match request.data.query_type {
            QueryRouteType::UnderlayNeighbors => Self::collect_neighbors(context)?.1,
        };

        let response = ProtocolMessage::QueryRouteRsp(ReqRspMessage {
            nonce: request.nonce,
            source_state_seq_nr: From::from(*context.uln_table().state_seq_nr()),
            data: RTableData { contacts },
            not_via: context.not_via().clone(),
            source_route: SourceRoute::from_reversed(request.source_route),
        });

        tracing::trace!(target: "vicinity_discovery", ?response, "send QueryRouteRsp");
        context
            .runtime()
            .send_message_via(response, underlay_destination.into());

        Ok(())
    }

    fn broadcast_uln_hello(&self, context: &C) {
        let hello = HelloMessage {
            source: *context.root_id(),
            source_state_seq_nr: From::from(*context.uln_table().state_seq_nr()),
        };

        tracing::trace!( target: "vicinity_discovery", ?hello, "broadcaste ULNHello");
        context.runtime().send_message_via(hello, Broadcast);
    }

    fn multicast_uln_hello_interface(context: &C, interface: InterfaceId) {
        let hello = HelloMessage {
            source: *context.root_id(),
            source_state_seq_nr: From::from(*context.uln_table().state_seq_nr()),
        };

        tracing::trace!(target: "vicinity_discovery", %interface, ?hello, "LL-Multicast ULNHello");
        context
            .runtime()
            .send_message_via(hello, Multicast(interface));
    }

    fn send_uln_disc_req(
        context: &C,
        destination: NodeId,
        underlay_destination: UnderlayNeighborId,
        nonce: Nonce,
    ) -> Result<(), VDError> {
        let (ssn, contacts) = Self::collect_neighbors(context)?;
        let request = ProtocolMessage::ULNDiscReq(ReqRspMessage {
            nonce,
            source_state_seq_nr: ssn.into(),
            data: RTableData { contacts },
            not_via: context.not_via().clone(),
            // Source route is ignored, as only underlay neighbors get these
            source_route: SourceRoute::from(Path::from([*context.root_id(), destination])),
        });

        tracing::trace!(target: "vicinity_discovery", ?request, "send ULNDiscReq");
        context
            .runtime()
            .send_message_via(request, UnderlayNeighbor(underlay_destination));
        Ok(())
    }

    fn send_uln_disc_rsp(
        context: &C,
        request: ReqRspMessage<RTableData>,
        underlay_destination: UnderlayNeighborId,
    ) -> Result<(), VDError> {
        let (ssn, contacts) = Self::collect_neighbors(context)?;
        let response = ProtocolMessage::ULNDiscRsp(ReqRspMessage {
            nonce: request.nonce,
            source_state_seq_nr: ssn.into(),
            data: RTableData { contacts },
            not_via: context.not_via().clone(),
            source_route: SourceRoute::from_reversed(request.source_route),
        });

        tracing::trace!(target: "vicinity_discovery", ?response, "send ULNDiscRsp");
        context
            .runtime()
            .send_message_via(response, UnderlayNeighbor(underlay_destination));

        Ok(())
    }

    fn collect_neighbors(context: &C) -> Result<(SafeStateSeqNr, Vec<Contact>), VDError> {
        let rt_lock = context.routing_table();
        let uln_lock = context.uln_table();

        let neighbors = uln_lock.keys().collect::<Vec<_>>();
        let mut contacts = Vec::with_capacity(neighbors.len());
        for uln_id in neighbors {
            let contact = rt_lock.contact(uln_id).cloned();
            if contact.is_none() {
                tracing::error!(
                    target: "vicinity_discovery",
                    underlay_neighbor = %uln_id,
                    "contact of underlay neighbor not found"
                );
                return Err(VDError::NeighborInconsistency);
            }
            contacts.push(contact.unwrap());
        }
        Ok((*uln_lock.state_seq_nr(), contacts))
    }
}

// manage requests
impl<C, const BUCKET_SIZE: usize> VicinityDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
    C::VicinityGraph: VicinityGraph,
{
    fn register_pending_req(
        &mut self,
        context: &C,
        destination: NodeId,
        expected_nonce: Nonce,
        initial_timeout: Duration,
    ) {
        let VDState::Running {
            timer_hooks,
            pending_reqs,
            ..
        } = &mut self.state
        else {
            panic!("VicinityDiscovery should be running");
        };

        pending_reqs.insert(
            destination,
            RequestState {
                timeouts: 1,
                timeout: initial_timeout,
                expected_nonce,
            },
        );

        // set timeout timer
        let timeout_timer_id = context
            .runtime()
            .register_rand_timer(self.config.uln_min_interval);
        timer_hooks.insert(timeout_timer_id, TimerHook::RepeatReq(destination));
    }

    fn init_query_route_req(&mut self, context: &C, path: Path) -> Result<(), VDError> {
        let destination = *path.last();
        let expected_nonce = Nonce::random();
        Self::send_query_route_req(context, path, expected_nonce)?;

        let initial_timeout = self.config.uln_discovery_rsp_initial_max_wait_time;
        tracing::trace!(
            target: "vicinity_discovery",
            %destination,
            timeout_ms = initial_timeout.as_millis(),
            "register pending QueryRouteReq",
        );
        self.register_pending_req(context, destination, expected_nonce, initial_timeout);

        Ok(())
    }

    fn init_uln_disc_req(
        &mut self,
        context: &C,
        destination: NodeId,
        underlay_neighbor: UnderlayNeighborId,
    ) -> Result<(), VDError> {
        let expected_nonce = Nonce::random();
        Self::send_uln_disc_req(context, destination, underlay_neighbor, expected_nonce)?;

        let initial_timeout = self.config.uln_discovery_rsp_initial_max_wait_time;
        tracing::trace!(
            target: "vicinity_discovery",
            %destination,
            timeout_ms = initial_timeout.as_millis(),
            "register pending ULNDiscReq",
        );
        self.register_pending_req(context, destination, expected_nonce, initial_timeout);

        Ok(())
    }

    // prohibit restart of pending requests

    fn init_new_uln_disc_req(
        &mut self,
        context: &C,
        node: NodeId,
        underlay_neighbor: UnderlayNeighborId,
    ) -> Result<bool, VDError> {
        let VDState::Running { pending_reqs, .. } = &self.state else {
            panic!("VicinityDiscovery should be running");
        };
        if pending_reqs.contains_key(&node) {
            return Ok(false);
        }

        self.init_uln_disc_req(context, node, underlay_neighbor)?;

        Ok(true)
    }

    fn init_new_query_route_req(&mut self, context: &C, path: Path) -> Result<bool, VDError> {
        let VDState::Running { pending_reqs, .. } = &self.state else {
            panic!("VicinityDiscovery should be running");
        };
        if pending_reqs.contains_key(path.last()) {
            return Ok(false);
        }

        self.init_query_route_req(context, path)?;

        Ok(true)
    }

    fn finalize_request<T: Debug>(&mut self, response: ReqRspMessage<T>) {
        let VDState::Running { pending_reqs, .. } = &mut self.state else {
            return;
        };

        let perceived_nonce = response.nonce;
        let nid = response.source();

        let hash_map::Entry::Occupied(entry) = pending_reqs.entry(*nid) else {
            tracing::warn!(
                target: "vicinity_discovery",
                source = %nid,
                reason = "no_pending_req",
                "unexpected response received",
            );
            return;
        };
        if entry.get().expected_nonce == perceived_nonce {
            tracing::debug!(
                target: "vicinity_discovery",
                source = %nid,
                request = ?entry.get(),
                reason = "response_received",
                "request complete",
            );
            entry.remove();
        } else {
            tracing::trace!(
                target: "vicinity_discovery",
                source = %nid,
                %perceived_nonce,
                request = ?entry.get(),
                reason = "unexpected_nonce",
                "ignore response",
            );
        }
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase for VicinityDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
    C::VicinityGraph: VicinityGraph + Debug,
{
    type State = VDState;

    fn start(&mut self, context: &C) -> Result<(), Self::Error> {
        let resync_timer_id = context
            .runtime()
            .register_rand_timer(self.config.resync_timeout);

        self.state = VDState::Running {
            resync_timer_id,
            timer_hooks: HashMap::default(),

            pending_reqs: HashMap::default(),
            interfaces: HashMap::default(),
        };

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

impl<C, const BUCKET_SIZE: usize> EventHandler for VicinityDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
    C::VicinityGraph: VicinityGraph + Debug,
{
    type Context = C;
    type Error = VDError;
    type Value = ();

    #[instrument(
        level = Level::TRACE,
        target = "vicinity_discovery",
        "vicinity_discovery",
        skip(self, context),
        fields(
            state = ?self.state,
            config = ?self.config
        )
    )]
    fn handle_event(&mut self, context: &C, event: UseCaseEvent) -> Result<(), Self::Error> {
        match (event.clone(), &mut self.state) {
            // ========== Vicinity Discovery - Query Route ==========
            (
                UseCaseEvent::Message(
                    ProtocolMessage::QueryRouteReq(request),
                    UnderlayNeighborSource::UnderlayNeighbor(underlay_source),
                ),
                _,
            ) => {
                tracing::trace!(
                    target: "vicinity_discovery",
                    source = %request.source(),
                    reason = "recv_query_route_req",
                    "send QueryRouteRsp"
                );
                self.send_query_route_rsp(context, request, underlay_source)
            }
            (UseCaseEvent::Message(ProtocolMessage::QueryRouteRsp(response), _), _) => {
                self.finalize_request(response);

                Ok(())
            }
            // ========== Underlay Neighbor Discovery ==========
            (
                UseCaseEvent::Message(
                    ProtocolMessage::ULNHello(HelloMessage {
                        source,
                        source_state_seq_nr: observed_ssn,
                        ..
                    }),
                    UnderlayNeighborSource::UnderlayNeighbor(underlay_source),
                ),
                VDState::Running {
                    pending_reqs,
                    interfaces,
                    ..
                },
            ) => {
                if !pending_reqs.contains_key(&source) {
                    tracing::trace!(
                        target: "vicinity_discovery",
                        %source,
                        reason = "pending_req",
                        "ignore ULNHello",
                    );
                }

                if !deterministic_heuristic(
                    context.root_id(),
                    &source,
                    self.config.heuristic_calculation_bits,
                ) {
                    let interface = &underlay_source.interface_id;

                    // still respond if wait time for our next ULNHello larger then heuristic_max_wait_time
                    match interfaces.get(interface) {
                        Some(InterfaceState { hello_interval })
                            if hello_interval > &self.config.heuristic_max_wait_time =>
                        {
                            // TODO: obtain *actual* wait time left using the UseCaseRuntime
                            // currently this an expensive operation by the UseCaseRuntime
                            // and we must save the TimerId in the InterfaceState

                            tracing::trace!(
                                target: "vicinity_discovery",
                                %source,
                                %interface,
                                deterministic_heuristic = "ignore",
                                current_hello_interval_ms = hello_interval.as_millis(),
                                heuristic_max_wait_time_ms = self.config.heuristic_max_wait_time.as_millis(),
                                exceeded_heuristic_max_wait_time = "true",
                                reason = "exceeded_heuristic_max_wait_time",
                                "ignore deterministic_heuristic",
                            );
                        }
                        None => {
                            // this is likely caused by the io-part
                            // not correctly publishing InterfaceUp events
                            tracing::warn!(
                                target: "vicinity_discovery",
                                %interface,
                                "no periodic ULNHellos on interface"
                            );

                            tracing::trace!(
                                target: "vicinity_discovery",
                                %source,
                                %interface,
                                deterministic_heuristic = "ignore",
                                reason = "no_periodic_interface_uln_hello",
                                "ignore deterministic_heuristic",
                            );
                        }
                        Some(InterfaceState { hello_interval }) => {
                            tracing::trace!(
                                target: "vicinity_discovery",
                                %source,
                                %interface,
                                deterministic_heuristic = "ignore",
                                current_hello_interval_ms = hello_interval.as_millis(),
                                heuristic_max_wait_time_ms = self.config.heuristic_max_wait_time.as_millis(),
                                exceeded_heuristic_max_wait_time = "false",
                                reason = "deterministic_heuristic",
                                "ignore ULNHello",
                            );
                            return Ok(());
                        }
                    }
                }

                match observed_ssn {
                    StateSeqNr::Invalid => {
                        panic!("Invalid StateSeqNr reached VicinityDiscovery use-case")
                    }
                    StateSeqNr::Reset => {
                        tracing::debug!(
                            target: "vicinity_discovery",
                            %source,
                            deterministic_heuristic = "answer",
                            reason = "ssn_reset",
                            "init underlay neighbor discovery",
                        );
                    }
                    StateSeqNr::Value(observed_ssn) => {
                        let vicinity_graph = context.vicinity_graph();
                        let vicinity_ssn = vicinity_graph.vicinity_ssn(&source);
                        if vicinity_ssn < Some(&observed_ssn) {
                            tracing::debug!(
                                target: "vicinity_discovery",
                                %source,
                                deterministic_heuristic = "answer",
                                vicinity_ssn = if let Some(vicinity_ssn) = vicinity_ssn { format!("{vicinity_ssn}") } else { "N/A".to_string()},
                                %observed_ssn,
                                reason = "vicinity_outdated",
                                "init underlay neighbor discovery",
                            );
                        }
                    }
                }

                self.init_uln_disc_req(context, source, underlay_source)?;
                Ok(())
            }
            (
                UseCaseEvent::Message(
                    ProtocolMessage::ULNDiscReq(req),
                    UnderlayNeighborSource::UnderlayNeighbor(underlay_src),
                ),
                _,
            ) => {
                tracing::trace!(
                    target: "vicinity_discovery",
                    source = %req.source(),
                    reason = "recv_uln_disc_req",
                    "send ULNDiscRsp"
                );
                Self::send_uln_disc_rsp(context, req, underlay_src)
            }
            (UseCaseEvent::Message(ProtocolMessage::ULNDiscRsp(response), _), _) => {
                self.finalize_request(response);

                Ok(())
            }
            // ========== Underlay Changes ==========
            // Not reacting to individual underlay neighbors but only to interfaces.
            (
                UseCaseEvent::UnderlayUpdate(UnderlayNeighborUpdate::InterfaceUp(interface)),
                VDState::Running {
                    interfaces,
                    timer_hooks,
                    ..
                },
            ) => {
                tracing::debug!(
                    target: "vicinity_discovery",
                    %interface,
                    reason = "interface_up",
                    "send immediate ULNHello",
                );
                Self::multicast_uln_hello_interface(context, interface);

                tracing::debug!(
                    target: "vicinity_discovery",
                    %interface,
                    reason = "interface_up",
                    "init periodic interface ULNHello",
                );
                let hello_timer_id = context
                    .runtime()
                    .register_rand_timer(self.config.uln_min_interval);
                interfaces.insert(
                    interface,
                    InterfaceState {
                        hello_interval: self.config.uln_min_interval,
                    },
                );
                timer_hooks.insert(hello_timer_id, TimerHook::SendInterfaceHello(interface));

                Ok(())
            }
            (
                UseCaseEvent::UnderlayUpdate(UnderlayNeighborUpdate::InterfaceDown(interface)),
                VDState::Running { interfaces, .. },
            ) => {
                tracing::debug!(
                    target: "vicinity_discovery",
                    %interface,
                    reason = "interface_down",
                    "stop periodic interface ULNHello",
                );
                interfaces.remove(&interface);

                Ok(())
            }
            // ========== Resynchronisation management ==========
            (
                UseCaseEvent::Timer(timer_id),
                VDState::Running {
                    resync_timer_id,
                    pending_reqs,
                    ..
                },
            ) if &timer_id == resync_timer_id => {
                let pending_syncs = pending_reqs.len();
                let max_pending_syncs = self.config.max_parallel_resync_count;
                let mut remaining_sync_capacity =
                    max_pending_syncs.saturating_sub(pending_reqs.len());
                if remaining_sync_capacity == 0 {
                    tracing::trace!(
                        target: "vicinity_discovery",
                        pending_syncs, max_pending_syncs, remaining_sync_capacity,
                        reason = "pending_sync_capacity_exhausted",
                        "not scheduling additional syncs",
                    );
                    return Ok(());
                }
                tracing::trace!(
                    target: "vicinity_discovery",
                    pending_syncs, max_pending_syncs, remaining_sync_capacity,
                    reason = "pending_sync_capacity_available",
                    "scheduling additional syncs",
                );

                let _schedule_syncs_span = tracing::debug_span!(
                    target: "vicinity_discovery",
                    "schedule_syncs",
                )
                .entered();

                let isolated_vicinity_nodes = {
                    let vg_lock = context.vicinity_graph();
                    let rt_lock = context.routing_table();
                    let uln_lock = context.uln_table();
                    let lowest_bucket = rt_lock.bucket(context.root_id());

                    // sync priority is:
                    // 1. underlay neighbors
                    // 2. nodes in the lowest bucket
                    // 3. other nodes in the vicinity graph to sync

                    // construct iterator with the given priority
                    // contains duplicates

                    let uln_sync = uln_lock
                        .keys()
                        // don't sync with unknown underlay neighbors because we either about to receive a ULNHello or dead
                        .filter(|uln| requires_sync(uln, vg_lock.deref()))
                        .copied();
                    let lowest_bucket_sync = lowest_bucket
                        .iter()
                        .filter_map(|contact| {
                            if requires_sync(contact.id(), vg_lock.deref()) {
                                Some(contact.id())
                            } else {
                                None
                            }
                        })
                        .copied();
                    let vicinity_sync = vg_lock
                        .deref()
                        .nodes()
                        .filter(|vicinity_node| requires_sync(vicinity_node, vg_lock.deref()));

                    let all_sync = uln_sync.chain(lowest_bucket_sync).chain(vicinity_sync);

                    let mut isolated_vicinity_nodes = false;
                    for nid in all_sync {
                        // init_new_* required since we can only access pending_reqs
                        // in one location because of the borrow rules

                        let new_sync = if let Some(underlay_neighbor) =
                            context.uln_table().get(&nid).copied()
                        {
                            self.init_new_uln_disc_req(context, nid, underlay_neighbor)?
                        } else {
                            let Some(path) = vg_lock.vicinity_path_to(nid) else {
                                tracing::trace!(
                                    target: "vicinity_discovery",
                                    %nid,
                                    reason = "unreachable_vicinity",
                                    "remove from vicinity graph"
                                );

                                // removal of node from the vicinity graph
                                // can't happen here because we still hold the vg_lock
                                isolated_vicinity_nodes = true;
                                continue;
                            };
                            if path.size() == 1 {
                                tracing::error!(
                                    target: "vicinity_discovery",
                                    node = %nid,
                                    "underlay vicinity node not in uln_table"
                                );
                                return Err(VDError::NeighborInconsistency);
                            }

                            self.init_new_query_route_req(context, path)?
                        };

                        if !new_sync {
                            tracing::trace!(
                                target: "vicinity_discovery",
                                node = %nid,
                                reason = "pending_sync",
                                "pass syncing with node",
                            );
                            continue;
                        }

                        tracing::debug!(
                            target: "vicinity_discovery",
                            pending_syncs = max_pending_syncs-remaining_sync_capacity, max_pending_syncs, remaining_sync_capacity,
                            uln = uln_lock.contains(&nid),
                            lowest_bucket = lowest_bucket.contains(&nid),
                            reason = "requires_sync",
                            node = %nid,
                            "initialized sync with node",
                        );

                        remaining_sync_capacity -= 1;
                        if remaining_sync_capacity == 0 {
                            tracing::trace!(
                                target: "vicinity_discovery",
                                pending_syncs = max_pending_syncs-remaining_sync_capacity, max_pending_syncs, remaining_sync_capacity,
                                reason = "pending_sync_capacity_exhausted",
                                "stop scheduling nodes for sync",
                            );
                            break;
                        }
                    }

                    isolated_vicinity_nodes
                };

                if isolated_vicinity_nodes {
                    // collecting not required for pruning
                    let _ = context.vicinity_graph_mut().retain_vicinity();
                }

                self.set_next_resync_timeout(context);
                Ok(())
            }
            (UseCaseEvent::Vicinity(VicinityEvent::SSNChanged), _) => {
                tracing::debug!(target: "vicinity_discovery", reason = "ssn_changed", "broadcast ULNHello");

                // TODO: add random delay to avoid node overload on synchronous resync attempts
                self.broadcast_uln_hello(context);
                Ok(())
            }
            // ========== Timeout Management ==========
            (
                UseCaseEvent::Timer(ref timer_id),
                VDState::Running {
                    timer_hooks,
                    interfaces,
                    pending_reqs,
                    ..
                },
            ) => {
                match timer_hooks.remove(timer_id) {
                    Some(TimerHook::RepeatReq(destination)) => {
                        let hash_map::Entry::Occupied(mut entry) = pending_reqs.entry(destination)
                        else {
                            // likely answered before timeout
                            tracing::trace!(
                                target: "vicinity_discovery",
                                %destination,
                                "not repeating canceled request",
                            );
                            return Ok(());
                        };
                        let uln_discovery_max_retries = self.config.uln_discovery_max_retries;
                        let req_state = entry.get_mut();
                        let timeouts = req_state.timeouts;
                        let timeout = req_state.timeout;

                        tracing::trace!(
                            target: "vicinity_discovery",
                            uln_discovery_max_retries,
                            timeouts,
                            %destination,
                            timeout_ms = timeout.as_millis(),
                            "request timed out",
                        );

                        if timeouts > uln_discovery_max_retries {
                            tracing::debug!(
                                target: "vicinity_discovery",
                                uln_discovery_max_retries,
                                timeouts,
                                %destination,
                                timeout_ms = timeout.as_millis(),
                                reason = "uln_discovery_max_retries_reached",
                                "remove stale vicinity node",
                            );
                            entry.remove();

                            // remove from structures
                            context.vicinity_graph_mut().remove(&destination);
                            //context.uln_table_mut().remove(&destination);
                            //if let Some(mut contact) =
                            //    context.routing_table_mut().contact_mut(&destination)
                            //{
                            //    *contact.state_mut() = ContactState::Invalid;
                            //}

                            return Ok(());
                        }

                        let resend_req_span = tracing::debug_span!(
                            target: "vicinity_discovery",
                            "resend_request",
                            %destination,
                            reason = "request_timed_out",
                            timeout_ms = field::Empty, // new timeout
                        )
                        .entered();

                        // check if still inside the vicinity
                        // e. g.: Link failure moves node outside the vicinity

                        // don't check vicinity_ssn < observed_ssn
                        // because update would finalize this request
                        // (unless a different nonce that wasn't ignored was used)
                        //
                        // if in the future we have advanced means inferring
                        // the nodes vicinity we should probably check here
                        // to avoid unnecessary request repeats

                        // underlay_neighbor => inside the vicinity
                        if let Some(underlay_neighbor) =
                            context.uln_table().get(&destination).copied()
                        {
                            Self::send_uln_disc_req(
                                context,
                                destination,
                                underlay_neighbor,
                                req_state.expected_nonce,
                            )?;
                        } else {
                            let mut vg_lock = context.vicinity_graph_mut();
                            let Some(path) = vg_lock.vicinity_path_to(destination) else {
                                tracing::trace!(
                                    target: "vicinity_discovery",
                                    %destination,
                                    reason = "unreachable_vicinity",
                                    "abort sending QueryRouteReq"
                                );
                                entry.remove();

                                tracing::trace!(
                                    target: "vicinity_discovery",
                                    %destination,
                                    reason = "unreachable_vicinity",
                                    "remove from vicinity graph"
                                );
                                vg_lock.remove(&destination);
                                return Ok(());
                            };
                            if path.size() == 1 {
                                tracing::error!(
                                    target: "vicinity_discovery",
                                    node = %destination,
                                    "underlay vicinity node not in uln_table"
                                );
                                return Err(VDError::NeighborInconsistency);
                            }

                            Self::send_query_route_req(context, path, req_state.expected_nonce)?;
                        }

                        // register new timeout timer
                        let timeout = timeout * 2;
                        resend_req_span.record("timeout_ms", timeout.as_millis());
                        let timeout_timer_id = context.runtime().register_rand_timer(timeout);
                        timer_hooks.insert(timeout_timer_id, TimerHook::RepeatReq(destination));

                        req_state.timeouts += 1;
                        req_state.timeout = timeout;
                    }
                    Some(TimerHook::SendInterfaceHello(interface)) => {
                        let Some(InterfaceState { hello_interval }) =
                            interfaces.get_mut(&interface)
                        else {
                            // likely because the interface went down
                            tracing::debug!(
                                target: "vicinity_discovery",
                                %interface,
                                reason = "no_periodic_interface_uln_hello",
                                "abort sending periodic ULNHello on interface",
                            );
                            return Ok(());
                        };

                        tracing::trace!(
                            target: "vicinity_discovery",
                            %interface,
                            "send periodic interface ULNHello",
                        );
                        Self::multicast_uln_hello_interface(context, interface);

                        *hello_interval = min(self.config.uln_max_interval, *hello_interval * 2);
                        tracing::trace!(
                            target: "vicinity_discovery",
                            %interface,
                            hello_interval_ms = hello_interval.as_millis(),
                            "schedule next periodic interface ULNHello",
                        );
                        let hello_timer_id = context.runtime().register_rand_timer(*hello_interval);
                        timer_hooks
                            .insert(hello_timer_id, TimerHook::SendInterfaceHello(interface));
                    }
                    None => {} // timer of other use-case
                }

                Ok(())
            }
            // ===================================================
            (UseCaseEvent::API(ApiEvent::VicinityGraph(sender)), _) => {
                let _ = sender.send(format!("{:#?}", context.vicinity_graph()));
                Ok(())
            }
            _ => Ok(()),
        }
    }
}
