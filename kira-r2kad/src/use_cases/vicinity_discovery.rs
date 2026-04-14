use crate::messaging::WireFormatMessage;
use std::cmp::min;
use std::collections::{HashMap, HashSet, hash_map};
use std::fmt::Debug;
use std::marker::PhantomData;
use std::num::NonZeroU8;
use std::ops::Deref;
use std::time::Duration;
use tracing::{Level, field, instrument};

use derive_more::derive::{Display, Error};

use crate::domain::{
    Contact, DEFAULT_BUCKET_SIZE, InterfaceId, NodeId, NotVia, Path, RoutingTable, SafeStateSeqNr,
    StateSeqNr, ULNTable,
    UnderlayNeighborDestination::{Broadcast, Multicast, UnderlayNeighbor},
    UnderlayNeighborId, UnderlayNeighborSource, UnderlayNeighborUpdate, VICINITY_RADIUS,
    VicinityGraph,
};
use crate::messaging::{
    CommonHeader, Nonce, ProtocolMessage, ProtocolMessageKind, QueryRouteReqData, QueryRouteType,
    RTableData, ReqRspMessage, source_route::SourceRoute,
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

    /// Initial maximum wait-time to receive the dedicated QueryRouteRsp.
    ///
    /// The wait-time is doubled between consecutive tries until
    /// [`query_route_discovery_max_retries`](Self::query_route_discovery_max_retries) is reached.
    pub query_route_discovery_rsp_initial_max_wait_time: Duration,
    /// Maximum retries to complete an QueryRoute handshake with a vicinity neighbor.
    pub query_route_discovery_max_retries: usize,

    /// Number of bits for the deterministic heuristic to consider for deciding which node should
    /// respond to the ULNHello message.
    pub heuristic_calculation_bits: NonZeroU8,
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
            uln_discovery_rsp_initial_max_wait_time: Duration::from_millis(250),
            uln_discovery_max_retries: 2,

            query_route_discovery_rsp_initial_max_wait_time: Duration::from_secs(3),
            query_route_discovery_max_retries: 2,

            heuristic_calculation_bits: NonZeroU8::new(32).unwrap(),
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
    #[display("Internal error")]
    InternalError,
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct InterfaceState {
    hello_interval: Duration,
}

/// Information about the pending response to a request.
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct RequestState {
    kind: ProtocolMessageKind,
    timeouts: usize,
    timeout: Duration,
    expected_nonce: Nonce,
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum TimerHook {
    TimeoutULNDiscReq(NodeId),
    TimeoutQueryRouteReq(NodeId),
    SendInterfaceHello(InterfaceId),
}

#[derive(Debug, Eq, PartialEq, Clone, Default)]
pub enum VDState {
    #[default]
    Initialized,
    Running {
        resync_timer_id: TimerId,
        timer_hooks: HashMap<TimerId, TimerHook>,

        pending_reqs: HashMap<(NodeId, ProtocolMessageKind), RequestState>,
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
fn deterministic_heuristic(self_id: &NodeId, other: &NodeId, num_bits: NonZeroU8) -> bool {
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

impl<C, const BUCKET_SIZE: usize> VicinityDiscovery<C, BUCKET_SIZE> {
    /// Create a new vicinity discovery use case in [VDState::Initialized].
    pub fn new(config: VicinityDiscoveryConfig) -> Self {
        if NodeId::BITS < config.heuristic_calculation_bits.get() {
            panic!(
                "Number of bits to use for the heuristic in VicinityDiscovery is greater than BITS of NodeId."
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
        // Convert contacts path to source route
        let source_route = SourceRoute::from(path);
        let next_hop = source_route.current_hop(); // :)
        let hop_count = source_route.size() - 1;

        assert_eq!(
            source_route.source(),
            context.root_id(),
            "SourceRoute of QueryRouteReq has to start at ourselves",
        );

        // Crucially Nodes _on_ the Vicinity Radius are also excluded
        // because we discover them using their neighbors inside the Vicinity
        assert!(
            hop_count < VICINITY_RADIUS,
            "shouldn't send QueryRouteReq outside the vicinity"
        );
        assert!(
            hop_count > 1,
            "shouldn't send QueryRouteReq to underlay neighbors"
        );

        // Get interface of route
        let Some(underlay_neighbor) = context.uln_table().get(next_hop).cloned() else {
            tracing::error!(
                target: "vicinity_discovery",
                ?source_route,
                reason = "neighbor_inconsistency",
                "abort sending QueryRouteReq"
            );
            return Err(VDError::NeighborInconsistency);
        };

        // Request only underlay Neighborhood of that Node
        let request = ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::QueryRouteReq,
                *context.root_id(),
                *source_route.destination(),
                Some(nonce.into()),
                Some(From::from(*context.uln_table().state_seq_nr())),
                context.uln_table().size(),
            ),
            data: QueryRouteReqData {
                query_type: QueryRouteType::UnderlayNeighbors,
            },
            not_via: context.not_via_state().iter().map(NotVia::from).collect(),
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
            QueryRouteType::UnderlayNeighbors => Self::collect_underlay_neighbors(context)?.1,
        };

        let response = ProtocolMessage::QueryRouteRsp(ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::QueryRouteRsp,
                *context.root_id(),
                *request.source(),
                Some(request.msg_id()),
                Some(From::from(*context.uln_table().state_seq_nr())),
                context.uln_table().size(),
            ),
            data: RTableData { contacts },
            not_via: context.not_via_state().iter().map(NotVia::from).collect(),
            source_route: SourceRoute::from_reversed(request.source_route),
        });

        tracing::trace!(target: "vicinity_discovery", ?response, "send QueryRouteRsp");
        context
            .runtime()
            .send_message_via(response, underlay_destination.into());

        Ok(())
    }

    fn broadcast_uln_hello(&self, context: &C) {
        let hello = ProtocolMessage::ULNHello(CommonHeader::new(
            ProtocolMessageKind::ULNHello,
            *context.root_id(),
            NodeId::ALL_NODES,
            None,
            Some(From::from(*context.uln_table().state_seq_nr())),
            context.uln_table().size(),
        ));

        tracing::trace!( target: "vicinity_discovery", ?hello, "broadcasting ULNHello");
        context.runtime().send_message_via(hello, Broadcast);
    }

    fn multicast_uln_hello_interface(context: &C, interface: InterfaceId) {
        let hello = ProtocolMessage::ULNHello(CommonHeader::new(
            ProtocolMessageKind::ULNHello,
            *context.root_id(),
            NodeId::ALL_NODES,
            None,
            Some(From::from(*context.uln_table().state_seq_nr())),
            context.uln_table().size(),
        ));

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
        let (ssn, contacts) = Self::collect_underlay_neighbors(context)?;
        let request = ProtocolMessage::ULNDiscReq(ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::ULNDiscReq,
                *context.root_id(),
                destination,
                Some(nonce.into()),
                Some(ssn.into()),
                context.uln_table().size(),
            ),
            data: RTableData { contacts },
            not_via: context.not_via_state().iter().map(NotVia::from).collect(),
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
        let (ssn, contacts) = Self::collect_underlay_neighbors(context)?;
        let response = ProtocolMessage::ULNDiscRsp(ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::ULNDiscRsp,
                *context.root_id(),
                *request.source(),
                Some(request.msg_id()),
                Some(ssn.into()),
                context.uln_table().size(),
            ),
            data: RTableData { contacts },
            not_via: context.not_via_state().iter().map(NotVia::from).collect(),
            source_route: SourceRoute::from_reversed(request.source_route),
        });

        tracing::trace!(target: "vicinity_discovery", ?response, "send ULNDiscRsp");
        context
            .runtime()
            .send_message_via(response, UnderlayNeighbor(underlay_destination));

        Ok(())
    }

    fn collect_underlay_neighbors(context: &C) -> Result<(SafeStateSeqNr, Vec<Contact>), VDError> {
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
        expected_kind: ProtocolMessageKind,
        initial_timeout: Duration,
        timeout_hook: TimerHook,
    ) {
        let VDState::Running {
            timer_hooks,
            pending_reqs,
            ..
        } = &mut self.state
        else {
            panic!("VicinityDiscovery should be running");
        };

        tracing::trace!(
            target: "vicinity_discovery",
            %destination,
            %expected_nonce,
            %expected_kind,
            timeout_ms = initial_timeout.as_millis(),
            "register pending request",
        );

        pending_reqs.insert(
            (destination, expected_kind),
            RequestState {
                kind: expected_kind,
                timeouts: 0,
                timeout: initial_timeout,
                expected_nonce,
            },
        );

        // set timeout timer
        let timeout_timer_id = context.runtime().register_rand_timer(initial_timeout);
        timer_hooks.insert(timeout_timer_id, timeout_hook);
    }

    fn init_query_route_req(&mut self, context: &C, path: Path) -> Result<(), VDError> {
        let destination = *path.last();
        let expected_nonce = Nonce::random();
        Self::send_query_route_req(context, path, expected_nonce)?;

        self.register_pending_req(
            context,
            destination,
            expected_nonce,
            ProtocolMessageKind::QueryRouteRsp,
            self.config.query_route_discovery_rsp_initial_max_wait_time,
            TimerHook::TimeoutQueryRouteReq(destination),
        );

        Ok(())
    }

    fn init_uln_disc_req(
        &mut self,
        context: &C,
        destination: NodeId,
        underlay_neighbor: UnderlayNeighborId,
    ) -> Result<(), VDError> {
        let expected_nonce = Nonce::random();

        tracing::trace!(
            target: "vicinity_discovery",
            from = %context.root_id(),
            to = %destination,
            underlay_neighbor = %underlay_neighbor,
            exp_nonce = %expected_nonce,
            "Sending ULNDiscReq",
        );

        Self::send_uln_disc_req(context, destination, underlay_neighbor, expected_nonce)?;

        self.register_pending_req(
            context,
            destination,
            expected_nonce,
            ProtocolMessageKind::ULNDiscRsp,
            self.config.uln_discovery_rsp_initial_max_wait_time,
            TimerHook::TimeoutULNDiscReq(destination),
        );

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
        if pending_reqs.contains_key(&(node, ProtocolMessageKind::ULNDiscRsp)) {
            return Ok(false);
        }

        self.init_uln_disc_req(context, node, underlay_neighbor)?;

        Ok(true)
    }

    fn init_new_query_route_req(&mut self, context: &C, path: Path) -> Result<bool, VDError> {
        let VDState::Running { pending_reqs, .. } = &self.state else {
            panic!("VicinityDiscovery should be running");
        };
        if pending_reqs.contains_key(&(*path.last(), ProtocolMessageKind::QueryRouteRsp)) {
            return Ok(false);
        }

        self.init_query_route_req(context, path)?;

        Ok(true)
    }

    fn process_ulndisc_reqrsp(&mut self, context: &C, req_or_rsp: &ReqRspMessage<RTableData>) {
        // update vicinity ssn in vicinity graph if required
        // sanity check for ULNDiscReq/Rsp messages, log error and ignore
        if req_or_rsp.source_route.size() != 2 {
            tracing::warn!(
                target: "vicinity_discovery",
                ?req_or_rsp,
                reason = "source route too long",
                "ULNDiscReq/Rsp expected to be received directly from ULN – ignored"
            );
            return;
        };
        // ULNDiscReq/Rsp confirms bidirectional reachability, so we need to add the
        // node to the vicinity graph (it will be added to the ULNtable in forward_protocol_message)

        let source_node = req_or_rsp.source();
        let mut old_source_neighbors: HashSet<_> =
            context.vicinity_graph().vicinity(source_node).collect();
        // remove own ID from neighbor's neighbors
        old_source_neighbors.remove(context.root_id());

        // look up ULN entry and update its vicinity SSN and last seen
        let mut vg = context.vicinity_graph_mut();
        let StateSeqNr::Value(synched_ssn) = req_or_rsp.state_seq_num() else {
            tracing::error!(
                target: "vicinity_discovery",
                ?req_or_rsp,
                reason = "state sequence number invalid",
                "underlay neighbor sent invalid StateSeqNr"
            );
            return;
        };
        // try to insert or update the underlay neighbor
        // note that the ULN node may exist in the vicinity graph already if inserted as neighbor of another ULN
        let inserted = vg
            .insert(*source_node, context.root_id(), synched_ssn)
            .unwrap_or_else(|err| {
                tracing::error!(target: "vicinity_discovery", ?req_or_rsp, reason = ?err, "error while inserting into vicinity");
                false
            });
        if let Some(source_vicinity_entry) = vg.entry_mut(source_node) {
            // this will also update observed ssn if the synched_ssn is newer
            if !inserted {
                source_vicinity_entry.update_synched_ssn(synched_ssn);
            }
            source_vicinity_entry.update_last_seen(context.runtime().current_time());
        }

        let source_neighbors = req_or_rsp
            .data
            .contacts
            .iter()
            .filter(|&c| c.path().size() == 1 && c.id() != context.root_id());

        // update links in vicinity graph
        for contact in source_neighbors {
            let inserted = vg
                .insert(*contact.id(), source_node, *contact.state_seq_nr())
                .unwrap_or_else(|_| {
                    panic!(
                        "insertion of edge {},{} failed with error",
                        *source_node,
                        contact.id()
                    )
                });
            if inserted {
                tracing::trace!(
                    target: "vicinity_discovery",
                    from = %req_or_rsp.source_route.source(),
                    to = %*contact.id(),
                    contact_ssn = %*contact.state_seq_nr(),
                    reason = "new edge",
                    "inserted new edge",
                );
            }
            old_source_neighbors.remove(contact.id());
        }

        // those left in old_source_neighbors are missing
        let removed_source_neighbors = old_source_neighbors;
        for removed_source_neighbor in removed_source_neighbors.iter() {
            assert!(
                vg.remove_edge(source_node, removed_source_neighbor),
                "removing edge of node's vicinity should change vicinity graph"
            );

            tracing::debug!(
                target: "vicinity_discovery",
                %source_node,
                node = %removed_source_neighbor,
                "removed edge from vicinity graph"
            );
        } // end for

        // clean up nodes moved outside the vicinity or got isolated
        // because of the removal of links
        if !removed_source_neighbors.is_empty() {
            for removed_node in vg.retain_vicinity() {
                tracing::debug!(
                    target: "vicinity_discovery",
                    %source_node,
                    node = %removed_node,
                    reason = "outside_vicinity",
                    "removed node from vicinity graph"
                );
            }
        }

        // now notify pre_compute_paths_and_pathids
        if vg.vicinity_changed() {
            context.runtime().broadcast_event(VicinityEvent::Changed);
        }
    }

    fn finalize_request(&mut self, response: ProtocolMessage) {
        let VDState::Running { pending_reqs, .. } = &mut self.state else {
            return;
        };

        let nid = response.source();

        let hash_map::Entry::Occupied(entry) = pending_reqs.entry((*nid, response.kind())) else {
            tracing::warn!(
                target: "vicinity_discovery",
                source = %nid,
                reason = "no_pending_req",
                "unexpected response received",
            );
            return;
        };
        if let Some(perceived_nonce) = response.msg_id() {
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
        } else {
            tracing::error!(
                target: "vicinity_discovery",
                source = %nid,
                request = ?entry.get(),
                reason = "response had no valid msgid",
                "ignore response",
            );
        }
    }

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
    fn handle_protocol_message(
        &mut self,
        context: &C,
        protocol_message: ProtocolMessage,
        underlay_source: UnderlayNeighborId,
    ) -> Result<(), VDError> {
        match protocol_message {
            // ========== Vicinity Discovery - Query Route Req ==========
            ProtocolMessage::QueryRouteReq(request,) => {
                tracing::trace!(
                    target: "vicinity_discovery",
                    source = %request.source(),
                    reason = "recv_query_route_req",
                    "send QueryRouteRsp"
                );
                // update observed SSN
                if request.source_route.size() == VICINITY_RADIUS
                    && let Some(entry) = context.vicinity_graph_mut().entry_mut(request.source())
                    && let StateSeqNr::Value(req_ssn) = request.state_seq_num()
                {
                    entry.update_observed_ssn(req_ssn);
                }
                self.send_query_route_rsp(context, request, underlay_source)
            }
            // ========== Vicinity Discovery - Query Route Rsp ==========
            ProtocolMessage::QueryRouteRsp(ref response,) => {
                // update synched SSN
                if response.source_route.size() == VICINITY_RADIUS
                    && let Some(entry) = context.vicinity_graph_mut().entry_mut(response.source())
                    && let StateSeqNr::Value(rsp_ssn) = response.common_header().state_seq_num()
                {
                    entry.update_synched_ssn(rsp_ssn);
                }

                self.finalize_request(protocol_message);

                Ok(())
            }
            // ========== Underlay Neighbor Discovery – ULNHello ==========
            // Process incoming ULNHello
            ProtocolMessage::ULNHello(common_header)
                // VDState::Running {
                //     pending_reqs,
                //     interfaces,
                //     ..
                // }
             => {
                let source = *common_header.src_node_id();
                let observed_ssn = common_header.state_seq_num();
                // in case a ULNHello is looped back somehow, ignore it, but log a warning
                if source == *context.root_id() {
                    tracing::warn!(
                        target: "vicinity_discovery",
                        %underlay_source,
                        reason = "received ULNHello from myself?!",
                        "ignore ULNHello",
                    );
                    // maybe return error instead
                    return Ok(());
                }

                // update entry in vicinity graph if necessary
                let mut new_neighbor = false;
                if let StateSeqNr::Value(seen_ssn) = observed_ssn {
                    let mut vg = context.vicinity_graph_mut();
                    if let Some(vg_entry) = vg.entry_mut(&source) {
                        vg_entry.update_observed_ssn(seen_ssn);
                        vg_entry.update_last_seen(context.runtime().current_time());

                        // it may be the case that the node was present as 2-hop neighbor already/still
                        // however, if it is now a ULN (again), we treat it as such
                        if !vg.is_direct_uln(&source) {
                            new_neighbor = true;
                        }
                    } else {
                        // node not present in vicinity graph
                        new_neighbor = true;
                    }
                }

                let VDState::Running { pending_reqs,
                                       interfaces, .. } = &mut self.state else {
                    return Err(VDError::InternalError);
                };
                if !new_neighbor && pending_reqs.contains_key(&(source,ProtocolMessageKind::ULNDiscRsp)) {
                    tracing::trace!(
                        target: "vicinity_discovery",
                        %source,
                        reason = "pending_req",
                        "ignore ULNHello",
                    );
                    return Ok(())
                }

                if !deterministic_heuristic(
                    context.root_id(),
                    &source,
                    self.config.heuristic_calculation_bits,
                ) {
                    // heuristic says: do not answer ULNHello
                    let interface = &underlay_source.interface_id;

                    // still respond if wait time for our next ULNHello larger than heuristic_max_wait_time
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
                } // endif: do not answer due to heuristic

                // only respond if news (newer SSN, reset, unknown neighbor...)
                match observed_ssn {
                    StateSeqNr::Invalid => {
                        panic!(
                            "Received invalid StateSeqNr from underlay neighbor in VicinityDiscovery use-case"
                        )
                    }
                    StateSeqNr::Reset => {
                        tracing::debug!(
                            target: "vicinity_discovery",
                            node = %source,
                            deterministic_heuristic = "answer",
                            reason = "ssn_reset",
                            "init underlay neighbor discovery",
                        );
                    }
                    StateSeqNr::Value(observed_ssn) => {
                        let vicinity_graph = context.vicinity_graph();
                        if let Some(entry) = vicinity_graph.entry(&source) && !new_neighbor {
                            let synched_ssn = entry.synched_ssn();
                            if synched_ssn.is_none() || synched_ssn < Some(&observed_ssn) {
                                tracing::debug!(
                                    target: "vicinity_discovery",
                                    node = %source,
                                    deterministic_heuristic = "answer",
                                    synched_ssn = if let Some(synched_ssn) = synched_ssn { format!("{synched_ssn}") } else { "N/A".to_string() },
                                    %observed_ssn,
                                    reason = "vicinity_outdated",
                                    "init underlay neighbor discovery",
                                );
                            } else {
                                return Ok(());
                            }
                        } else {
                            tracing::debug!(
                                target: "vicinity_discovery",
                                node = %source,
                                deterministic_heuristic = "answer",
                                synched_ssn = "N/A",
                                %observed_ssn,
                                reason = "new",
                                "init underlay neighbor discovery",
                            );
                        }
                    }
                }

                // pre_compute_paths_and_pathids will be notified by subsequent ULNDiscReq/RSp exchange

                // send ULNDiscReq back
                self.init_uln_disc_req(context, source, underlay_source)?;
                Ok(())
            }

            // ========== Underlay Neighbor Discovery – ULNDiscReq ==========
            // process ULNDiscReq
            ProtocolMessage::ULNDiscReq(req) => {
                tracing::trace!(
                    target: "vicinity_discovery",
                    source = %req.source(),
                    reason = "recv_uln_disc_req",
                    "send ULNDiscRsp"
                );
                self.process_ulndisc_reqrsp(context, &req);
                Self::send_uln_disc_rsp(context, req, underlay_source)
            }

            // ========== Underlay Neighbor Discovery – ULNDiscRsp ==========
            // Process ULNDiscRsp
            ProtocolMessage::ULNDiscRsp(ref ulndiscrsp) => {
                self.process_ulndisc_reqrsp(context, ulndiscrsp);

                self.finalize_request(protocol_message);

                Ok(())
            }

            _ => { // other messages are not of interest for vicinity discovery
                Ok(())
            }
        } // end match protocol message
    }
}

// complex actions
impl<C, const BUCKET_SIZE: usize> VicinityDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
    C::VicinityGraph: VicinityGraph,
{
    fn schedule_syncs(&mut self, context: &C) -> Result<(), VDError> {
        let VDState::Running { pending_reqs, .. } = &self.state else {
            panic!("VicinityDiscovery should be running");
        };

        let pending_syncs = pending_reqs.len();
        let max_pending_syncs = self.config.max_parallel_resync_count;
        let mut remaining_sync_capacity = max_pending_syncs.saturating_sub(pending_reqs.len());
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
            let uln_lock = context.uln_table();

            // sync priority is:
            // 1. underlay neighbors
            // 2. other nodes in the vicinity graph to sync

            // construct iterator with the given priority
            let uln_sync = uln_lock.keys().copied();
            let vicinity_sync = vg_lock.deref().nodes();
            let sync_candidates = uln_sync.chain(vicinity_sync);

            let mut isolated_vicinity_nodes = false;
            for sync_candidate in sync_candidates {
                // can't check for pending reqs in loop
                let new_sync = if let Some(underlay_neighbor) =
                    uln_lock.get(&sync_candidate).copied()
                {
                    // check if sync is required
                    let Some(entry) = vg_lock.entry(&sync_candidate) else {
                        // don't sync with unknown underlay neighbors because we either about to receive a ULNHello or dead

                        tracing::trace!(
                            target: "vicinity_discovery",
                            node = %sync_candidate,
                            reason = "uln_unknown",
                            expl = "waiting for ULNHello instead",
                            "ignore sync candidate",
                        );
                        continue;
                    };

                    let synched_ssn = entry.synched_ssn();
                    let observed_ssn = entry.observed_ssn();
                    if synched_ssn == Some(observed_ssn) {
                        tracing::trace!(
                            target: "vicinity_discovery",
                            node = %sync_candidate,
                            synched_ssn = if let Some(synched_ssn) = synched_ssn { format!("{synched_ssn}") } else { "N/A".to_string() },
                            %observed_ssn,
                            reason = "no_news",
                            "ignore sync candidate",
                        );
                        continue;
                    }

                    if synched_ssn > Some(observed_ssn) {
                        tracing::error!(
                            target: "vicinity_discovery",
                            node = %sync_candidate,
                            synched_ssn = if let Some(synched_ssn) = synched_ssn { format!("{synched_ssn}") } else { "N/A".to_string() },
                            %observed_ssn,
                            reason = "internal inconsistency",
                            "synched_ssn newer than observed_ssn",
                        );
                        panic!(
                            "for node {:?} synched_ssn newer {:?} than observed_ssn {:?}",
                            sync_candidate, synched_ssn, observed_ssn
                        );
                    }

                    self.init_new_uln_disc_req(context, sync_candidate, underlay_neighbor)?
                } else {
                    // check if sync is required
                    let entry = vg_lock
                        .entry(&sync_candidate)
                        .expect("vicinity node without entry");
                    let synched_ssn = entry.synched_ssn();
                    let observed_ssn = entry.observed_ssn();
                    if synched_ssn == Some(observed_ssn) {
                        tracing::trace!(
                            target: "vicinity_discovery",
                            node = %sync_candidate,
                            synched_ssn = if let Some(synched_ssn) = synched_ssn { format!("{synched_ssn}") } else { "N/A".to_string() },
                            %observed_ssn,
                            reason = "no_news",
                            "ignore sync candidate",
                        );
                        continue;
                    }

                    assert!(
                        synched_ssn < Some(observed_ssn),
                        "synched_ssn newer than observed_ssn"
                    );

                    let Some(path) = vg_lock.vicinity_path_to(sync_candidate) else {
                        tracing::trace!(
                            target: "vicinity_discovery",
                            node = %sync_candidate,
                            reason = "unreachable_vicinity",
                            "remove from vicinity graph"
                        );

                        // removal of node from the vicinity graph
                        // can't happen here because we still hold the vg_lock
                        isolated_vicinity_nodes = true;
                        continue;
                    };
                    if path.size() == 2 {
                        // sync candidate is a direct neighbor, so skip it here
                        //
                        // probable causes:
                        // 1. neighborhood update of node included us
                        // 2. actual underlay neighbor not in uln_table

                        // tracing::error!(
                        //     target: "vicinity_discovery",
                        //     node = %sync_candidate,
                        //     "underlay vicinity node not in uln_table"
                        // );
                        // return Err(VDError::NeighborInconsistency);
                        continue;
                    }

                    self.init_new_query_route_req(context, path)?
                };
                if !new_sync {
                    tracing::trace!(
                        target: "vicinity_discovery",
                        node = %sync_candidate,
                        reason = "pending_sync",
                        "pass syncing with node",
                    );
                    continue;
                }

                tracing::debug!(
                    target: "vicinity_discovery",
                    pending_syncs = max_pending_syncs-remaining_sync_capacity, max_pending_syncs, remaining_sync_capacity,
                    uln = uln_lock.contains(&sync_candidate),
                    reason = "requires_sync",
                    node = %sync_candidate,
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
            for removed_node in context.vicinity_graph_mut().retain_vicinity() {
                tracing::debug!(
                    target: "vicinity_discovery",
                    node = %removed_node,
                    reason = "outside_vicinity",
                    "removed node from vicinity graph"
                );
            }
        }

        Ok(())
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
                    message,
                    UnderlayNeighborSource::UnderlayNeighbor(underlay_source),
                ),
                _,
            ) => self.handle_protocol_message(context, message, underlay_source),
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
                    resync_timer_id, ..
                },
            ) if &timer_id == resync_timer_id => {
                self.schedule_syncs(context)?;
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
                    Some(TimerHook::TimeoutULNDiscReq(destination)) => {
                        let hash_map::Entry::Occupied(mut entry) =
                            pending_reqs.entry((destination, ProtocolMessageKind::ULNDiscRsp))
                        else {
                            // likely answered before timeout
                            tracing::trace!(
                                target: "vicinity_discovery",
                                node = %destination,
                                "not repeating canceled request",
                            );
                            return Ok(());
                        };
                        let uln_discovery_max_retries = self.config.uln_discovery_max_retries;
                        let req_state = entry.get_mut();
                        let timeouts = req_state.timeouts + 1;
                        let timeout = req_state.timeout;

                        tracing::trace!(
                            target: "vicinity_discovery",
                            uln_discovery_max_retries,
                            timeouts,
                            %destination, node = %destination,
                            timeout_ms = timeout.as_millis(),
                            "request timed out",
                        );

                        if timeouts > uln_discovery_max_retries {
                            tracing::debug!(
                                target: "vicinity_discovery",
                                uln_discovery_max_retries,
                                timeouts,
                                node = %destination,
                                timeout_ms = timeout.as_millis(),
                                reason = "uln_discovery_max_retries_reached",
                                "abort syncing with node",
                            );
                            entry.remove();

                            // remove from structures
                            if context.vicinity_graph_mut().remove(&destination) {
                                tracing::debug!(
                                    target: "vicinity_discovery",
                                    node = %destination,
                                    reason = "sync_timeout",
                                    "removed node from vicinity graph"
                                );
                            }

                            //context.uln_table_mut().remove(&destination);
                            //if let Some(mut contact) =
                            //    context.routing_table_mut().contact_mut(&destination)
                            //{
                            //    *contact.state_mut() = ContactState::Invalid;
                            //}

                            return Ok(());
                        }

                        let Some(underlay_neighbor) =
                            context.uln_table().get(&destination).copied()
                        else {
                            // e. g.: Link failure moves node outside the vicinity
                            tracing::debug!(
                                target: "vicinity_discovery",
                                node = %destination,
                                timeout_ms = timeout.as_millis(),
                                reason = "no_uln_table_entry",
                                "abort ULNDisc syncing with node",
                            );
                            entry.remove();

                            // don't remove node from vicinity graph because it is
                            // - connected via another node
                            // - not in the vicinity graph in the first place (initial handshake)
                            // - removed by FailureHandling because unconnected after link failure
                            return Ok(());
                        };

                        let resend_req_span = tracing::debug_span!(
                            target: "vicinity_discovery",
                            "resend_request",
                            node = %destination,
                            %destination,
                            reason = "request_timed_out",
                            timeout_ms = field::Empty, // new timeout
                        )
                        .entered();

                        // don't check synched_ssn < observed_ssn
                        // because update would finalize this request
                        //
                        // (unless a different nonce that wasn't ignored was used)
                        //
                        // if in the future we have advanced means inferring
                        // the nodes vicinity we should probably check here
                        // to avoid unnecessary request repeats

                        Self::send_uln_disc_req(
                            context,
                            destination,
                            underlay_neighbor,
                            req_state.expected_nonce,
                        )?;

                        // register new timeout timer
                        let timeout = timeout * 2;
                        resend_req_span.record("timeout_ms", timeout.as_millis());
                        let timeout_timer_id = context.runtime().register_rand_timer(timeout);
                        timer_hooks
                            .insert(timeout_timer_id, TimerHook::TimeoutULNDiscReq(destination));

                        req_state.timeouts += 1;
                        req_state.timeout = timeout;
                    }
                    Some(TimerHook::TimeoutQueryRouteReq(destination)) => {
                        let hash_map::Entry::Occupied(mut entry) =
                            pending_reqs.entry((destination, ProtocolMessageKind::QueryRouteRsp))
                        else {
                            // likely answered before timeout
                            tracing::trace!(
                                target: "vicinity_discovery",
                                node = %destination,
                                "not repeating canceled request",
                            );
                            return Ok(());
                        };
                        let query_route_discovery_max_retries =
                            self.config.query_route_discovery_max_retries;
                        let req_state = entry.get_mut();
                        let timeouts = req_state.timeouts + 1;
                        let timeout = req_state.timeout;

                        tracing::trace!(
                            target: "vicinity_discovery",
                            query_route_discovery_max_retries,
                            timeouts,
                            %destination, node = %destination,
                            timeout_ms = timeout.as_millis(),
                            "request timed out",
                        );

                        if timeouts > query_route_discovery_max_retries {
                            tracing::debug!(
                                target: "vicinity_discovery",
                                query_route_discovery_max_retries,
                                timeouts,
                                node = %destination,
                                timeout_ms = timeout.as_millis(),
                                reason = "query_route_discovery_max_retries_reached",
                                "abort QueryRoute syncing with node",
                            );
                            entry.remove();

                            // don't remove 2-hop node from vicinity graph on timeout
                            // because we should get the most updated information about our 2-hop
                            // neighbors by our 1-hop neighbors (ULN) and should trust them more.

                            // in the worst case the deletion will cause a missing
                            // path in the vicinity graph that is never recovered
                            // because the node didn't manage to respond in time once

                            return Ok(());
                        }

                        let vg_lock = context.vicinity_graph();
                        let Some(path) = vg_lock.vicinity_path_to(destination) else {
                            tracing::debug!(
                                target: "vicinity_discovery",
                                node = %destination,
                                reason = "unreachable_vicinity",
                                "abort QueryRoute syncing with node"
                            );
                            entry.remove();
                            return Ok(());
                        };
                        assert!(
                            path.size() <= VICINITY_RADIUS,
                            "vicinity graph generated path outside vicinity"
                        );

                        if path.size() == 2 {
                            // probably because we since discovered it as a underlay neighbor
                            //
                            // Note:
                            // we _only_ abort if its two way handshake is complete
                            // and makes it into the vicinity graph.
                            // Just consulting the uln_table won't suffice.
                            tracing::debug!(
                                target: "vicinity_discovery",
                                node = %destination,
                                reason = "underlay_neighbor",
                                "abort QueryRoute syncing with node"
                            );
                            entry.remove();
                            return Ok(());
                        }

                        let resend_req_span = tracing::debug_span!(
                            target: "vicinity_discovery",
                            "resend_request",
                            node = %destination,
                            %destination,
                            reason = "request_timed_out",
                            timeout_ms = field::Empty, // new timeout
                        )
                        .entered();

                        Self::send_query_route_req(context, path, req_state.expected_nonce)?;
                        // register new timeout timer
                        let timeout = timeout * 2;
                        resend_req_span.record("timeout_ms", timeout.as_millis());
                        let timeout_timer_id = context.runtime().register_rand_timer(timeout);
                        timer_hooks.insert(
                            timeout_timer_id,
                            TimerHook::TimeoutQueryRouteReq(destination),
                        );

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
