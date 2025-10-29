use std::cmp::min;
use std::collections::{HashMap, hash_map};
use std::fmt::Debug;
use std::marker::PhantomData;
use std::num::NonZeroUsize;
use std::ops::Deref;
use std::time::Duration;
use tracing::{Level, instrument};

use derive_more::derive::{Display, Error};

use crate::domain::UnderlayNeighborDestination::{Broadcast, Multicast, UnderlayNeighbor};
use crate::domain::{
    Contact, DEFAULT_BUCKET_SIZE, InterfaceId, NodeId, Path, RoutingTable, ULNTable,
    UnderlayNeighborId, UnderlayNeighborSource, UnderlayNeighborUpdate, VicinityGraph, node_id,
};
use crate::domain::{ContactState, VICINITY_RADIUS};
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
    timout_duration: Duration,
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

impl<C, const BUCKET_SIZE: usize> VicinityDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
    C::VicinityGraph: VicinityGraph,
{
    fn send_query_route_req(context: &C, node: NodeId, nonce: Nonce) -> Result<bool, VDError> {
        let vg = context.vicinity_graph();
        if !requires_sync(&node, vg.deref()) {
            tracing::trace!(target: "vicinity_discovery", "Ignoring vicinity update: No need to sync");
            return Ok(false);
        }
        let Some(path) = vg.vicinity_path_to(node) else {
            // cause could be changed vicinity which leaves node isolated or moved it outside the vicinity
            // but still present in the vicinity graph for some reason
            tracing::trace!(target: "vicinity_discovery", "Ignoring vicinity update: Not reachable by the root inside the vicinity");
            return Ok(false);
        };

        // Underlay Neighbors and Nodes outside of the Vicinity are not included
        //
        // Crucially Nodes _on_ the Vicinity Radius are also excluded
        // because we discover them using their neighbors inside the Vicinity
        assert!(
            path.size() < VICINITY_RADIUS,
            "Vicinity paths should be inside the vicinity"
        );
        if path.size() == 1 {
            tracing::trace!(target: "vicinity_discovery", "Ignoring vicinity update: Underlay neighbor or not in vicinity radius");
            return Ok(false);
        }

        // Convert contacts path to source route
        let mut route = SourceRoute::from(path.clone());
        route.push_front(*context.root_id());

        // Get interface of route
        let neighbor_port = context.uln_table().get(path.first()).cloned();
        if neighbor_port.is_none() {
            tracing::error!(
                target: "vicinity_discovery",
                "Temporary inconsistency: Valid contacts path starts with invalid underlay neighbor {}",
                path.first()
            );
            return Err(VDError::NeighborInconsistency);
        }

        // Request only underlay Neighborhood of that Node
        let request = ReqRspMessage {
            nonce,
            source_state_seq_nr: From::from(*context.uln_table().state_seq_nr()),
            data: QueryRouteReqData {
                query_type: QueryRouteType::UnderlayNeighbors,
            },
            not_via: context.not_via().clone(),
            source_route: route,
        };

        tracing::trace!(target: "vicinity_discovery", "Sending message {request:?}");

        context
            .runtime()
            .send_message(request, context.uln_table().deref());
        Ok(true)
    }

    fn send_query_route_rsp(
        &self,
        context: &C,
        request: ReqRspMessage<QueryRouteReqData>,
    ) -> Result<(), VDError> {
        let contacts = match request.data.query_type {
            QueryRouteType::UnderlayNeighbors => {
                let un_lock = context.uln_table();
                let rt_lock = context.routing_table();

                let result: Vec<Contact> = un_lock
                    .keys()
                    .filter_map(|id| {
                        let contact = rt_lock.contact(id).cloned();
                        // TODO: return VDError when try_collect is stable
                        if contact.is_none() {
                            tracing::error!(
                                target: "vicinity_discovery",
                                underlay_neighbor=%id,
                                "No contact found for underlay neighbor"
                            );
                        }

                        contact
                    })
                    .collect();
                result
            }
        };

        let message = ProtocolMessage::QueryRouteRsp(ReqRspMessage {
            nonce: request.nonce,
            source_state_seq_nr: From::from(*context.uln_table().state_seq_nr()),
            data: RTableData { contacts },
            not_via: context.not_via().clone(),
            source_route: SourceRoute::from_reversed(request.source_route),
        });

        tracing::trace!(target: "vicinity_discovery", "Sending: {message:?}");

        context
            .runtime()
            .send_message(message, context.uln_table().deref());

        Ok(())
    }

    fn broadcast_uln_hello(&self, context: &C) {
        let message = HelloMessage {
            source: *context.root_id(),
            source_state_seq_nr: From::from(*context.uln_table().state_seq_nr()),
        };

        log::trace!(target: "vicinity_discovery", "Broadcasting ULNHello");
        context.runtime().send_message_via(message, Broadcast);
    }

    fn multicast_uln_hello_interface(context: &C, interface: InterfaceId) {
        let message = HelloMessage {
            source: *context.root_id(),
            source_state_seq_nr: From::from(*context.uln_table().state_seq_nr()),
        };

        tracing::trace!(target: "vicinity_discovery", %interface, "LL-Multicast ULNHello");

        context
            .runtime()
            .send_message_via(message, Multicast(interface));
    }

    fn send_uln_disc_req(
        context: &C,
        destination: NodeId,
        underlay_destination: UnderlayNeighborId,
        nonce: Nonce,
    ) -> bool {
        if context
            .vicinity_graph()
            .observed_ssn(&destination)
            .is_some() // underlay neighbors are only added to the vicinity graph on ULNDiscReqRsp
            && !requires_sync(&destination, context.vicinity_graph().deref())
        {
            tracing::trace!(
                target: "vicinity_discovery",
                %destination,
                "Not responding to Hello from unchanged underlay neighbor as we received an unexpected state sequence number"
            );
            return false;
        }

        // Answer with a ULNDiscReq to ensure bidirectional connectivity
        let uln_contacts = context
            .uln_table()
            .iter()
            .filter_map(|(id, _)| context.routing_table().contact(id).cloned())
            .collect::<Vec<_>>();

        let message = ProtocolMessage::ULNDiscReq(ReqRspMessage {
            nonce,
            source_state_seq_nr: From::from(*context.uln_table().state_seq_nr()),
            data: RTableData {
                contacts: uln_contacts,
            },
            not_via: context.not_via().clone(),
            // Source route is ignored, as only underlay neighbors get these
            source_route: SourceRoute::from(Path::from([*context.root_id(), destination])),
        });

        tracing::trace!(
            target: "vicinity_discovery",
            "Sending message: {message:?}"
        );

        context
            .runtime()
            .send_message_via(message, UnderlayNeighbor(underlay_destination));

        true
    }

    fn send_uln_disc_rsp(
        context: &C,
        request: ReqRspMessage<RTableData>,
        underlay_destination: UnderlayNeighborId,
    ) -> Result<(), VDError> {
        // Note: Locks will be released at end of curly braces
        let (ssn, contacts) = {
            let rt_lock = context.routing_table();
            let uln_lock = context.uln_table();

            let neighbors = uln_lock.keys().collect::<Vec<_>>();
            let mut contacts = Vec::with_capacity(neighbors.len());
            for uln_id in neighbors {
                let contact = rt_lock.contact(uln_id).cloned();
                if contact.is_none() {
                    tracing::error!(target: "vicinity_discovery", "No contact found for underlay neighbor {uln_id}");
                    return Err(VDError::NeighborInconsistency);
                }
                contacts.push(contact.unwrap());
            }
            (*uln_lock.state_seq_nr(), contacts)
        };

        let response = ProtocolMessage::ULNDiscRsp(ReqRspMessage {
            nonce: request.nonce,
            source_state_seq_nr: ssn.into(),
            data: RTableData { contacts },
            not_via: context.not_via().clone(),
            source_route: SourceRoute::from_reversed(request.source_route),
        });

        tracing::trace!(target: "vicinity_discovery", "Sending: {response:?}");

        context
            .runtime()
            .send_message_via(response, UnderlayNeighbor(underlay_destination));

        Ok(())
    }
}

impl<C, const BUCKET_SIZE: usize> VicinityDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
    C::VicinityGraph: VicinityGraph,
{
    fn init_query_route_req(&mut self, context: &C, node: NodeId) -> Result<(), VDError> {
        let VDState::Running {
            timer_hooks,
            pending_reqs,
            ..
        } = &mut self.state
        else {
            return Ok(());
        };

        let expected_nonce = Nonce::random();
        if !Self::send_query_route_req(context, node, expected_nonce)? {
            return Ok(());
        }
        pending_reqs.insert(
            node,
            RequestState {
                timeouts: 1,
                timout_duration: self.config.uln_discovery_rsp_initial_max_wait_time,
                expected_nonce,
            },
        );

        // TODO: figure out if we should react to this event or just sync
        //       via the periodic resync
        let timeout_timer_id = context
            .runtime()
            .register_rand_timer(self.config.uln_min_interval);
        timer_hooks.insert(timeout_timer_id, TimerHook::RepeatReq(node));
        Ok(())
    }

    fn init_uln_disc_req(
        &mut self,
        context: &C,
        node: NodeId,
        underlay_neighbor: UnderlayNeighborId,
    ) -> Result<(), VDError> {
        let VDState::Running {
            timer_hooks,
            pending_reqs,
            ..
        } = &mut self.state
        else {
            return Ok(());
        };

        let expected_nonce = Nonce::random();
        if !Self::send_uln_disc_req(context, node, underlay_neighbor, expected_nonce) {
            return Ok(());
        }
        pending_reqs.insert(
            node,
            RequestState {
                timeouts: 1,
                timout_duration: self.config.uln_discovery_rsp_initial_max_wait_time,
                expected_nonce,
            },
        );

        let timeout_timer_id = context
            .runtime()
            .register_rand_timer(self.config.uln_min_interval);
        timer_hooks.insert(timeout_timer_id, TimerHook::RepeatReq(node));

        Ok(())
    }

    fn init_new_uln_disc_req(
        &mut self,
        context: &C,
        node: NodeId,
        underlay_neighbor: UnderlayNeighborId,
    ) -> Result<bool, VDError> {
        let VDState::Running { pending_reqs, .. } = &self.state else {
            return Ok(false);
        };
        if pending_reqs.contains_key(&node) {
            return Ok(false);
        }

        self.init_uln_disc_req(context, node, underlay_neighbor)?;
        Ok(true)
    }

    fn init_new_query_route_req(&mut self, context: &C, node: NodeId) -> Result<bool, VDError> {
        let VDState::Running { pending_reqs, .. } = &self.state else {
            return Ok(false);
        };
        if pending_reqs.contains_key(&node) {
            return Ok(false);
        }

        self.init_query_route_req(context, node)?;
        Ok(true)
    }
}

impl<C, const BUCKET_SIZE: usize> VicinityDiscovery<C, BUCKET_SIZE> {
    fn finalize_request<T: Debug>(&mut self, response: ReqRspMessage<T>) {
        let VDState::Running { pending_reqs, .. } = &mut self.state else {
            return;
        };

        let perceived_nonce = response.nonce;
        let nid = response.source();

        let hash_map::Entry::Occupied(entry) = pending_reqs.entry(*nid) else {
            return;
        };
        if entry.get().expected_nonce == perceived_nonce {
            entry.remove();
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
            (UseCaseEvent::Message(ProtocolMessage::QueryRouteReq(request), _), _) => {
                if request.destination() != context.root_id() {
                    return Ok(());
                }

                self.send_query_route_rsp(context, request)
            }
            (UseCaseEvent::Message(ProtocolMessage::QueryRouteRsp(response), _), _) => {
                self.finalize_request(response);

                Ok(())
            }
            // ========== Underlay Neighbor Discovery ==========
            (
                UseCaseEvent::Message(
                    ProtocolMessage::ULNHello(HelloMessage { source, .. }),
                    UnderlayNeighborSource::UnderlayNeighbor(underlay_source),
                ),
                VDState::Running {
                    pending_reqs,
                    interfaces,
                    ..
                },
            ) if !pending_reqs.contains_key(&source) => {
                if !deterministic_heuristic(
                    context.root_id(),
                    &source,
                    self.config.heuristic_calculation_bits,
                ) {
                    let interface = &underlay_source.interface_id;
                    let Some(InterfaceState { hello_interval }) = interfaces.get(interface) else {
                        tracing::error!(
                            target: "vicinity_discovery",
                            %interface,
                            "No periodic ULNHello setup for interface but received ULNHello");

                        return self.init_uln_disc_req(context, source, underlay_source);
                    };

                    // TODO: obtain *actual* wait time left using the UseCaseRuntime
                    // currently this an expensive operation by the UseCaseRuntime
                    // and we must save the TimerId in the InterfaceState

                    // still respond if wait time is unacceptably large
                    if hello_interval > &self.config.heuristic_max_wait_time {
                        tracing::debug!(
                            target: "vicinity_discovery",
                            %source,
                            %interface,
                            heuristic_max_wait_time=format!("{} s",self.config.heuristic_max_wait_time.as_secs_f64()),
                            "Responding to Hello because heuristic_max_wait_time was reached on the interface.");
                    } else {
                        tracing::trace!(target: "vicinity_discovery", %source, "Heuristic: Not responding to Hello");
                        return Ok(());
                    }
                }

                self.init_uln_disc_req(context, source, underlay_source)
            }
            (
                UseCaseEvent::Message(
                    ProtocolMessage::ULNDiscReq(req),
                    UnderlayNeighborSource::UnderlayNeighbor(underlay_src),
                ),
                _,
            ) => {
                if req.destination() != context.root_id() {
                    return Ok(());
                }
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
                Self::multicast_uln_hello_interface(context, interface);
                assert!(
                    interfaces
                        .insert(
                            interface,
                            InterfaceState {
                                hello_interval: self.config.uln_min_interval,
                            },
                        )
                        .is_none()
                );

                let hello_timer_id = context
                    .runtime()
                    .register_rand_timer(self.config.uln_min_interval);
                timer_hooks.insert(hello_timer_id, TimerHook::SendInterfaceHello(interface));

                Ok(())
            }
            (
                UseCaseEvent::UnderlayUpdate(UnderlayNeighborUpdate::InterfaceDown(interface)),
                VDState::Running { interfaces, .. },
            ) => {
                assert!(interfaces.remove(&interface).is_some());

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
                let mut unused_sync_capability = self
                    .config
                    .max_parallel_resync_count
                    .saturating_sub(pending_reqs.len());
                if unused_sync_capability == 0 {
                    return Ok(());
                }

                let vg = context.vicinity_graph();
                let rt = context.routing_table();
                let uln_table = context.uln_table();

                // sync priority is:
                // 1. underlay neighbors
                // 2. nodes in the lowest bucket
                // 3. other nodes in the vicinity graph to sync

                // construct iterator with the given priority
                // WITHOUT eliminating duplicates

                let uln_sync = uln_table
                    .keys()
                    .filter(|uln| requires_sync(uln, vg.deref()))
                    .copied();
                let lowest_bucket_sync = rt
                    .bucket(context.root_id())
                    .iter()
                    .filter_map(|c| {
                        if requires_sync(c.id(), vg.deref()) {
                            Some(c.id())
                        } else {
                            None
                        }
                    })
                    .copied();
                let vicinity_sync = vg
                    .deref()
                    .nodes()
                    .filter(|vicinity_node| requires_sync(vicinity_node, vg.deref()));

                let all_sync = uln_sync.chain(lowest_bucket_sync).chain(vicinity_sync);

                for nid in all_sync {
                    let new_sync = if let Some(underlay_neighbor) = uln_table.get(&nid) {
                        self.init_new_uln_disc_req(context, nid, *underlay_neighbor)?
                    } else {
                        self.init_new_query_route_req(context, nid)?
                    };
                    // a sync request is already pending
                    // but we want to initiate sync requests to new destinations
                    // so we try the next node to be synced
                    if !new_sync {
                        continue;
                    }

                    unused_sync_capability -= 1;
                    // stop if we exhausted the unused parallel sync capabilities
                    if unused_sync_capability == 0 {
                        break;
                    }
                }

                self.set_next_resync_timeout(context);
                // TODO: cleanup stale vicinity graph entries
                Ok(())
            }
            (UseCaseEvent::Vicinity(VicinityEvent::SSNChanged), _) => {
                tracing::trace!(target: "vicinity_discovery", "Changed SSN detected.");
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
                            // request was answered before timeout
                            return Ok(());
                        };

                        if entry.get().timeouts > self.config.uln_discovery_max_retries {
                            tracing::debug!(
                                target: "vicinity_discovery",
                                uln_discovery_max_retries=self.config.uln_discovery_max_retries,
                                node=%destination,
                                "Considering vicinity node dead after uln_discovery_max_retries reached.",
                            );
                            entry.remove();

                            // remove from structures
                            context.vicinity_graph_mut().remove(&destination);
                            context.uln_table_mut().remove(&destination);
                            if let Some(mut contact) =
                                context.routing_table_mut().contact_mut(&destination)
                            {
                                *contact.state_mut() = ContactState::Invalid;
                            }

                            return Ok(());
                        }

                        let sent = if let Some(underlay_neighbor) =
                            context.uln_table().get(&destination).copied()
                        {
                            Self::send_uln_disc_req(
                                context,
                                destination,
                                underlay_neighbor,
                                entry.get().expected_nonce,
                            )
                        } else {
                            Self::send_query_route_req(
                                context,
                                destination,
                                entry.get().expected_nonce,
                            )?
                        };

                        // check if we still have to send requests
                        if !sent {
                            entry.remove();
                            return Ok(());
                        }

                        let timeout_interval = min(
                            self.config.uln_discovery_rsp_initial_max_wait_time,
                            entry.get().timout_duration * 2,
                        );
                        let timeout_timer_id =
                            context.runtime().register_rand_timer(timeout_interval);
                        timer_hooks.insert(timeout_timer_id, TimerHook::RepeatReq(destination));

                        entry.get_mut().timeouts += 1;
                    }
                    Some(TimerHook::SendInterfaceHello(interface)) => {
                        let Some(InterfaceState { hello_interval }) =
                            interfaces.get_mut(&interface)
                        else {
                            // interface went down between interval
                            return Ok(());
                        };

                        Self::multicast_uln_hello_interface(context, interface);

                        *hello_interval = min(self.config.uln_max_interval, *hello_interval * 2);
                        let hello_timer_id = context.runtime().register_rand_timer(*hello_interval);
                        timer_hooks
                            .insert(hello_timer_id, TimerHook::SendInterfaceHello(interface));
                    }
                    None => {}
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
