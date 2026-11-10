use std::{
    cell::Ref,
    cmp::min,
    collections::{
        HashMap,
        HashSet,
        hash_map,
    },
    fmt::Debug,
    marker::PhantomData,
    num::NonZeroU8,
    ops::Deref,
    time::Duration,
};

use derive_more::derive::{
    Display,
    Error,
};
use tracing::{
    Level,
    field,
    instrument,
};

use crate::{
    domain::{
        Contact,
        DEFAULT_BUCKET_SIZE,
        InterfaceId,
        NodeId,
        Nonce,
        ProtocolMessage,
        ProtocolMessageKind,
        RoutingTable,
        SafeStateSeqNr,
        SourceRoute,
        StateSeqNr,
        ULNState,
        ULNTable,
        UnderlayNeighborDestination::{
            Broadcast,
            Multicast,
            UnderlayNeighbor,
        },
        UnderlayNeighborId,
        UnderlayNeighborSource,
        UnderlayNeighborUpdate,
        VICINITY_RADIUS,
        VicinityGraph,
        protocol_message::{
            CommonHeader,
            QueryRouteReqData,
            QueryRouteType,
            RTableData,
            ReqRspMessage,
            ULNReqRspMessage,
            WireFormatMessage,
        },
    },
    use_cases::{
        ApiEvent,
        EventHandler,
        TimerId,
        UseCase,
        UseCaseContext,
        UseCaseEvent,
        UseCaseRuntime,
        UseCaseState,
        VicinityEvent,
    },
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
    pub uln_discovery_rsp_check_max_wait_time: Duration,
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

    /// Timeout duration for checking for dead ULNs
    pub stale_check_timeout: Duration,
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
            uln_discovery_rsp_check_max_wait_time: Duration::from_millis(100),
            uln_discovery_max_retries: 2,

            query_route_discovery_rsp_initial_max_wait_time: Duration::from_secs(3),
            query_route_discovery_max_retries: 2,

            heuristic_calculation_bits: NonZeroU8::new(32).unwrap(),
            heuristic_max_wait_time: Duration::from_secs(1),
            resync_timeout: Duration::from_millis(100),
            stale_check_timeout: Duration::from_millis(2000),
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
    /// current timeout timer; timers with another id are outdated
    timer_id: TimerId,
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum TimerHook {
    TimeoutCheckForStaleULNs,
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
    let (mut delta, _) = other_bits.overflowing_sub(own_bits);
    let mut mask = u128::MAX;
    mask <<= u128::BITS - num_bits.get() as u32;
    mask >>= u128::BITS - num_bits.get() as u32;
    delta &= mask;
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
    C::UnderlayNeighborTable:
        ULNTable + Deref<Target = HashMap<NodeId, (UnderlayNeighborId, ULNState)>>,
    C::VicinityGraph: VicinityGraph,
{
    fn send_query_route_req(
        context: &C,
        source_route: SourceRoute,
        nonce: Nonce,
    ) -> Result<(), VDError> {
        // Convert contacts path to source route
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
        let Some((underlay_neighbor, _)) = context.uln_table().get(next_hop).cloned() else {
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
                Some(nonce),
                *context.uln_table().state_seq_nr(),
                context.uln_table().size(),
            ),
            data: QueryRouteReqData {
                query_type: QueryRouteType::UnderlayNeighbors,
            },
            not_via: None,
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
                *context.uln_table().state_seq_nr(),
                context.uln_table().size(),
            ),
            data: RTableData { contacts },
            not_via: None,
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
            *context.uln_table().state_seq_nr(),
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
            *context.uln_table().state_seq_nr(),
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
        let request = ProtocolMessage::ULNDiscReq(ULNReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::ULNDiscReq,
                *context.root_id(),
                destination,
                Some(nonce),
                ssn,
                context.uln_table().size(),
            ),
            data: RTableData { contacts },
        });

        tracing::trace!(target: "vicinity_discovery", ?request, "send ULNDiscReq");
        context
            .runtime()
            .send_message_via(request, UnderlayNeighbor(underlay_destination));
        Ok(())
    }

    fn send_uln_disc_rsp(
        context: &C,
        request: ULNReqRspMessage<RTableData>,
        underlay_destination: UnderlayNeighborId,
    ) -> Result<(), VDError> {
        let (ssn, contacts) = Self::collect_underlay_neighbors(context)?;
        let response = ProtocolMessage::ULNDiscRsp(ULNReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::ULNDiscRsp,
                *context.root_id(),
                *request.source(),
                Some(request.msg_id()),
                ssn,
                context.uln_table().size(),
            ),
            data: RTableData { contacts },
        });

        tracing::trace!(target: "vicinity_discovery", ?response, "send ULNDiscRsp");
        context
            .runtime()
            .send_message_via(response, UnderlayNeighbor(underlay_destination));

        Ok(())
    }

    fn collect_underlay_neighbors(context: &C) -> Result<(SafeStateSeqNr, Vec<Contact>), VDError> {
        let rt_lock = context.routing_table();
        let uln_lock: Ref<'_, C::UnderlayNeighborTable> = context.uln_table();

        // we collect ULNs with bidirectional connectivity only
        let neighbors = uln_lock
            .iter()
            .filter(|(_, (_, uln_state))| matches!(*uln_state, ULNState::Bidirectional))
            .map(|(uln_node_id, _)| uln_node_id)
            .collect::<Vec<_>>();
        let mut contacts = Vec::with_capacity(neighbors.len());
        // NOTE that unless handshake is complete some of the nodes in the uln_table
        // may not be a regular contact yet. However, since we only filter for bidirectional
        // entries, every ULN should be present in the RoutingTable
        for uln_node_id in neighbors {
            if let Some(contact) = rt_lock.contact(uln_node_id).cloned() {
                contacts.push(contact);
            } else {
                tracing::error!(
                    target: "vicinity_discovery",
                    underlay_neighbor = %uln_node_id,
                    "contact of confirmed underlay neighbor not found"
                );
                return Err(VDError::NeighborInconsistency);
            }
        }
        Ok((*uln_lock.state_seq_nr(), contacts))
    }

    fn check_for_stale_ulns(&mut self, context: &C) {
        tracing::debug!(
            target: "vicinity_discovery",
            reason = "check_for_stale_ULNs",
        );
        let vg = context.vicinity_graph();
        for node_id in vg.vicinity(context.root_id()) {
            // for every contact that has not sent anything for a longer time,
            // start liveness check with ULNDiscReq
            let uln_entry = vg
                .entry(&node_id)
                .expect("vicinity graph is inconsistent, entry must be present for neighbor");
            if let Some(last_seen) = uln_entry.last_seen()
                && (context.runtime().current_time() - last_seen) > self.config.uln_max_interval
                && let Some((underlay_neighbor, _)) = context.uln_table().get(&node_id).cloned()
            {
                let result = self.send_uln_disc_req_now(
                    context,
                    node_id,
                    underlay_neighbor,
                    self.config.uln_discovery_rsp_check_max_wait_time,
                );
                if result.is_err() {
                    tracing::error!(
                        target: "vicinity_discovery",
                        ?result,
                        reason = "check_for_stale_ULNs: send_uln_disc_req_now returned error for {node_id}",
                    );
                }
            }
        }
    }
}

// manage requests
impl<C, const BUCKET_SIZE: usize> VicinityDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable:
        ULNTable + Deref<Target = HashMap<NodeId, (UnderlayNeighborId, ULNState)>>,
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

        // set timeout timer
        let timeout_timer_id = context.runtime().register_rand_timer(initial_timeout);
        timer_hooks.insert(timeout_timer_id, timeout_hook);

        pending_reqs.insert(
            (destination, expected_kind),
            RequestState {
                kind: expected_kind,
                timeouts: 0,
                timeout: initial_timeout,
                expected_nonce,
                timer_id: timeout_timer_id,
            },
        );
    }

    fn init_query_route_req(
        &mut self,
        context: &C,
        source_route: SourceRoute,
    ) -> Result<(), VDError> {
        let destination = *source_route.destination();
        let expected_nonce = Nonce::random();
        Self::send_query_route_req(context, source_route, expected_nonce)?;

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
        timeout: Duration,
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
            timeout,
            TimerHook::TimeoutULNDiscReq(destination),
        );

        Ok(())
    }

    /// Sends a ULNDiscReq to `destination` immediately.
    ///
    /// If a ULNDiscReq is pending already, it is resent with the same nonce, so that a late
    /// response to it is still accepted, and its retry timer is restarted with the shorter of its
    /// current and the given timeout. The retry count is kept, so the request still ends after
    /// `uln_discovery_max_retries`.
    fn send_uln_disc_req_now(
        &mut self,
        context: &C,
        destination: NodeId,
        underlay_neighbor: UnderlayNeighborId,
        timeout: Duration,
    ) -> Result<(), VDError> {
        let VDState::Running {
            timer_hooks,
            pending_reqs,
            ..
        } = &mut self.state
        else {
            panic!("VicinityDiscovery should be running");
        };
        let Some(req_state) = pending_reqs.get_mut(&(destination, ProtocolMessageKind::ULNDiscRsp))
        else {
            return self.init_uln_disc_req(context, destination, underlay_neighbor, timeout);
        };

        Self::send_uln_disc_req(
            context,
            destination,
            underlay_neighbor,
            req_state.expected_nonce,
        )?;

        // replace the retry timer
        context.runtime().remove_timer(req_state.timer_id);
        timer_hooks.remove(&req_state.timer_id);
        let timeout = timeout.min(req_state.timeout);
        let timeout_timer_id = context.runtime().register_rand_timer(timeout);
        timer_hooks.insert(timeout_timer_id, TimerHook::TimeoutULNDiscReq(destination));
        req_state.timer_id = timeout_timer_id;
        req_state.timeout = timeout;

        tracing::trace!(
            target: "vicinity_discovery",
            node = %destination,
            timeouts = req_state.timeouts,
            timeout_ms = timeout.as_millis(),
            reason = "pending_req",
            "resent pending ULNDiscReq immediately",
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

        self.init_uln_disc_req(
            context,
            node,
            underlay_neighbor,
            self.config.uln_discovery_rsp_initial_max_wait_time,
        )?;

        Ok(true)
    }

    fn init_new_query_route_req(
        &mut self,
        context: &C,
        source_route: SourceRoute,
    ) -> Result<bool, VDError> {
        let VDState::Running { pending_reqs, .. } = &self.state else {
            panic!("VicinityDiscovery should be running");
        };
        if pending_reqs.contains_key(&(
            *source_route.destination(),
            ProtocolMessageKind::QueryRouteRsp,
        )) {
            return Ok(false);
        }

        self.init_query_route_req(context, source_route)?;

        Ok(true)
    }

    fn process_ulndisc_reqrsp(&mut self, context: &C, req_or_rsp: &ULNReqRspMessage<RTableData>) {
        // update vicinity ssn in vicinity graph if required
        // ULNDiscReq/Rsp confirms bidirectional reachability, so we need to add the
        // node to the vicinity graph (it will be added to the ULNtable in handling ULNHello or ULNDiscReq)

        let source_node = req_or_rsp.source();
        let mut old_source_neighbors: HashSet<_> =
            context.vicinity_graph().vicinity(source_node).collect();
        // remove own ID from neighbor's neighbors
        old_source_neighbors.remove(context.root_id());

        // look up ULN entry and update its vicinity SSN and last seen
        let mut vg = context.vicinity_graph_mut();
        let StateSeqNr::Value(synced_ssn) = req_or_rsp.state_seq_num() else {
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
        vg.insert(*source_node, context.root_id(), synced_ssn)
            .expect("insertion of ULN into vicinity graph failed");

        let source_vicinity_entry = vg
            .entry_mut(source_node)
            .expect("node should be available in vicinity graph directly after insertion");
        source_vicinity_entry.update_synced_ssn(synced_ssn);
        source_vicinity_entry.update_last_seen(context.runtime().current_time());

        let source_neighbors = req_or_rsp
            .data
            .contacts
            .iter()
            .filter(|&c| c.path().unwrap().size() == 1 && c.id() != context.root_id());

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
                    from = %req_or_rsp.source(),
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
                    removed_node = %removed_node,
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
            ProtocolMessage::QueryRouteReq(request) => {
                tracing::trace!(
                    target: "vicinity_discovery",
                    source = %request.source(),
                    reason = "recv_query_route_req",
                    "send QueryRouteRsp"
                );
                if !request.exact() {
                    tracing::warn!(target: "vicinity_discovery", "Received QueryRouteReq without exact flag");
                }

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
                // update synced SSN for two-hop neighbors (must not be ULNs then)
                let mut vg_mut = context.vicinity_graph_mut();
                if !vg_mut.is_direct_uln(response.source())
                    && response.source_route.size() == VICINITY_RADIUS
                    && let Some(entry) = vg_mut.entry_mut(response.source())
                    && let StateSeqNr::Value(rsp_ssn) = response.common_header().state_seq_num()
                {
                    entry.update_synced_ssn(rsp_ssn);
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
                let observed_ssn_from_msg = common_header.state_seq_num();
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

                // add source node to ULNTable
                { // required due to scope ULNTable borrow
                    // add source node to ULNTable as we probably need to send back a reply
                    let mut uln_table = context.uln_table_mut();
                    if !uln_table.contains(&source) {
                        if let Some(replaced) = uln_table.insert(source, underlay_source) {
                            // Not allowed to happen as lock is held
                            log::warn!(
                                target: "uln_table",
                                "Overwritten ulnid mapping for '{source}' from '{underlay_source}' to '{replaced}' but checked before"
                            );
                        } else {
                            log::debug!(target: "uln_table", "Inserted neighbor '{source}' at ulnid '{underlay_source}'");
                        }
                    }
                }

                // update entry in vicinity graph if necessary
                let mut new_neighbor = false;
                if let StateSeqNr::Value(seen_ssn) = observed_ssn_from_msg {
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
                match observed_ssn_from_msg {
                    StateSeqNr::Invalid => {
                        panic!(
                            "Received invalid StateSeqNr from underlay neighbor in VicinityDiscovery use case"
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
                            let last_synced_ssn = entry.synced_ssn();
                            if last_synced_ssn.is_none() || last_synced_ssn < Some(&observed_ssn) {
                                tracing::debug!(
                                    target: "vicinity_discovery",
                                    node = %source,
                                    deterministic_heuristic = "answer",
                                    last_synced_ssn = if let Some(last_synced_ssn) = last_synced_ssn { format!("{last_synced_ssn}") } else { "N/A".to_string() },
                                    %observed_ssn,
                                    reason = "vicinity_outdated",
                                    "init underlay neighbor discovery",
                                );
                            } else {
                                return Ok(());
                            }
                        } else {
                            // no entry in the vicinity graph or new neighbor
                            tracing::debug!(
                                target: "vicinity_discovery",
                                node = %source,
                                deterministic_heuristic = "answer",
                                synced_ssn = "N/A",
                                %observed_ssn,
                                reason = "new",
                                "init underlay neighbor discovery",
                            );
                        }
                    }
                }

                // pre_compute_paths_and_pathids will be notified by subsequent ULNDiscReq/RSp exchange

                // send ULNDiscReq back (immediately, even if one is pending already)
                self.send_uln_disc_req_now(context, source, underlay_source, self.config.uln_discovery_rsp_initial_max_wait_time)?;
                Ok(())
            }

            // ========== Underlay Neighbor Discovery – ULNDiscReq ==========
            // process ULNDiscReq (this is normally a reaction to a ULNHello)
            ProtocolMessage::ULNDiscReq(req) => {
                // add source node to ULNTable
                { // required due to scope ULNTable borrow
                    let source = req.source();
                    // add source node to ULNTable as we probably need to send back a reply
                    let mut uln_table = context.uln_table_mut();
                    uln_table.insert(*source, underlay_source);
                    if uln_table.confirm(*source, underlay_source) {
                        log::debug!(target: "uln_table", "Confirmed neighbor '{source}' at ulnid '{underlay_source}'");
                    }
                }
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
                // need to confirm bidirectional reachability (ULNHello should have done the initial insert of the entry)
                { // required due to scope ULNTable borrow
                    let source = ulndiscrsp.source();
                    // add source node to ULNTable as we probably need to send back a reply
                    let mut uln_table = context.uln_table_mut();
                    if uln_table.confirm(*source, underlay_source) {
                        log::debug!(target: "uln_table", "Confirmed neighbor '{source}' at ulnid '{underlay_source}'");
                    }
                }
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
    C::UnderlayNeighborTable:
        ULNTable + Deref<Target = HashMap<NodeId, (UnderlayNeighborId, ULNState)>>,
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

            // construct iterator with the given priority ULNs before two hop neighbors
            let uln_sync = uln_lock.keys().copied();
            let vicinity_sync = vg_lock.deref().nodes();
            let mut seen = HashSet::new();
            let sync_candidates = uln_sync.chain(vicinity_sync).filter(|x| seen.insert(*x));

            let mut isolated_vicinity_nodes = false;
            for sync_candidate in sync_candidates {
                // can't check for pending reqs in loop
                let new_sync = if let Some((underlay_neighbor, _)) =
                    uln_lock.get(&sync_candidate).copied()
                {
                    // check if sync (ULNDisReq/Rsp) with neighbor is required
                    let Some(entry) = vg_lock.entry(&sync_candidate) else {
                        // node is missing in  vicinity_graph:
                        // it may be present in uln_table but not yet in vicinity_graph
                        // don't sync with unconfirmed underlay neighbors because
                        // we are either about to receive a ULNHello or the node is unreachable/disconnected

                        tracing::trace!(
                            target: "vicinity_discovery",
                            node = %sync_candidate,
                            reason = "node missing in vicinity graph",
                            expl = "waiting for ULNHello instead",
                            "ignore sync candidate",
                        );
                        continue;
                    };

                    // node is present in vicinity graph
                    let synced_ssn = entry.synced_ssn();
                    let observed_ssn = entry.observed_ssn();
                    if synced_ssn == Some(observed_ssn) {
                        tracing::trace!(
                            target: "vicinity_discovery",
                            node = %sync_candidate,
                            synced_ssn = if let Some(synced_ssn) = synced_ssn { format!("{synced_ssn}") } else { "N/A".to_string() },
                            %observed_ssn,
                            reason = "no_news",
                            "ignore sync candidate",
                        );
                        continue;
                    }

                    if synced_ssn > Some(observed_ssn) {
                        tracing::error!(
                            target: "vicinity_discovery",
                            node = %sync_candidate,
                            synced_ssn = if let Some(synced_ssn) = synced_ssn { format!("{synced_ssn}") } else { "N/A".to_string() },
                            %observed_ssn,
                            reason = "internal inconsistency",
                            "synced_ssn newer than observed_ssn",
                        );
                        panic!(
                            "for node {:?} synced_ssn newer {:?} than observed_ssn {:?}",
                            sync_candidate, synced_ssn, observed_ssn
                        );
                    }
                    // if arrived here, synced_ssn < observed_ssn, so ask neighbor for current state
                    self.init_new_uln_disc_req(context, sync_candidate, underlay_neighbor)?
                } else {
                    // sync_candidate is not a ULN, so it is two hops away
                    // check if sync with QueryRouteReq is required
                    let entry = vg_lock
                        .entry(&sync_candidate)
                        .expect("vicinity node without entry");
                    let synced_ssn = entry.synced_ssn();
                    let observed_ssn = entry.observed_ssn();
                    if synced_ssn == Some(observed_ssn) {
                        tracing::trace!(
                            target: "vicinity_discovery",
                            node = %sync_candidate,
                            synced_ssn = if let Some(synced_ssn) = synced_ssn { format!("{synced_ssn}") } else { "N/A".to_string() },
                            %observed_ssn,
                            reason = "no_news",
                            "ignore sync candidate",
                        );
                        continue;
                    }

                    assert!(
                        synced_ssn < Some(observed_ssn),
                        "synced_ssn newer than observed_ssn"
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

                    // send QueryRouteReq
                    let source_route = SourceRoute::try_from(path)
                        .expect("Paths of vicinity graph include start of path");
                    self.init_new_query_route_req(context, source_route)?
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
            let mut vg = context.vicinity_graph_mut();
            for removed_node in vg.retain_vicinity() {
                tracing::debug!(
                    target: "vicinity_discovery",
                    node = %removed_node,
                    reason = "isolated node",
                    "removed node from vicinity graph"
                );
            }
            if vg.vicinity_changed() {
                context.runtime().broadcast_event(VicinityEvent::Changed);
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
    C::UnderlayNeighborTable:
        ULNTable + Deref<Target = HashMap<NodeId, (UnderlayNeighborId, ULNState)>>,
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

        let stale_check_timer_id = context
            .runtime()
            .register_periodic_timer(self.config.stale_check_timeout);

        if let VDState::Running { timer_hooks, .. } = &mut self.state {
            timer_hooks.insert(stale_check_timer_id, TimerHook::TimeoutCheckForStaleULNs);
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
    C::UnderlayNeighborTable:
        ULNTable + Deref<Target = HashMap<NodeId, (UnderlayNeighborId, ULNState)>>,
    C::VicinityGraph: VicinityGraph + Debug,
{
    type Context = C;
    type Error = VDError;
    type Value = ();

    #[instrument(
        target = "vicinity_discovery",
        "vicinity_discovery",
        skip(self, context, event),
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
                VDState::Running { interfaces, .. },
            ) => {
                if !interfaces.contains_key(&underlay_source.interface_id) {
                    tracing::debug!(
                        target: "vicinity_discovery",
                        %underlay_source,
                        reason = "received message on disabled interface",
                        "ignoring message {:?}",
                        message
                    );
                    return Ok(());
                }
                self.handle_protocol_message(context, message, underlay_source)
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
                // periodic timer: the runtime fires it again under the same id, so keep its hook
                if matches!(
                    timer_hooks.get(timer_id),
                    Some(TimerHook::TimeoutCheckForStaleULNs)
                ) {
                    self.check_for_stale_ulns(context);
                    return Ok(());
                }
                match timer_hooks.remove(timer_id) {
                    Some(TimerHook::TimeoutCheckForStaleULNs) => {
                        unreachable!("periodic stale check is handled above")
                    }
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
                        if req_state.timer_id != *timer_id {
                            // timer was replaced, e.g. by an immediate resend
                            tracing::trace!(
                                target: "vicinity_discovery",
                                node = %destination,
                                "ignore outdated request timer",
                            );
                            return Ok(());
                        }
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

                        // no response after uln_discovery_max_retries, remove link to ULN
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

                            // remove direct link from vicinity graph
                            let mut vg = context.vicinity_graph_mut();
                            if vg.remove_edge(context.root_id(), &destination) {
                                tracing::debug!(
                                    target: "vicinity_discovery",
                                    node = %destination,
                                    reason = "sync_timeout",
                                    "removed direct link to node from vicinity graph"
                                );
                            }

                            context.uln_table_mut().remove(&destination);

                            // clean up nodes moved outside the vicinity or isolated by the link removal
                            for removed_node in vg.retain_vicinity() {
                                tracing::debug!(
                                    target: "vicinity_discovery",
                                    node = %removed_node,
                                    reason = "outside_vicinity",
                                    "removed node from vicinity graph"
                                );
                            }

                            // let pre_compute_paths_and_pathids remove the now invalid paths
                            if vg.vicinity_changed() {
                                context.runtime().broadcast_event(VicinityEvent::Changed);
                            }

                            let mut rt = context.routing_table_mut();
                            // if last link has been removed, the node will be deleted also from RT
                            if vg.entry(&destination).is_none() {
                                rt.remove(&destination);
                            }
                            // invalidate all contacts whose path starts with the lost ULN,
                            // including its own contact if the direct link was active
                            // (the contact may also be in the two-hop vicinity)
                            for node in rt.invalidate_contacts_via(&HashSet::from([destination])) {
                                tracing::debug!(
                                    target: "vicinity_discovery",
                                    %node,
                                    via = %destination,
                                    reason = "sync_timeout",
                                    "invalidated contact",
                                );
                            }

                            return Ok(());
                        }

                        // timeout for receiving ProtocolMessageKind::ULNDiscRsp
                        // send another ULNDiscReq
                        let Some((underlay_neighbor, _)) =
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

                        // don't check synced_ssn < observed_ssn
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

                        // register new timer with doubled timeout
                        let timeout = timeout * 2;
                        resend_req_span.record("timeout_ms", timeout.as_millis());
                        let timeout_timer_id = context.runtime().register_rand_timer(timeout);
                        timer_hooks
                            .insert(timeout_timer_id, TimerHook::TimeoutULNDiscReq(destination));

                        req_state.timeouts += 1;
                        req_state.timeout = timeout;
                        req_state.timer_id = timeout_timer_id;
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
                        if req_state.timer_id != *timer_id {
                            // timer was replaced, e.g. by an immediate resend
                            tracing::trace!(
                                target: "vicinity_discovery",
                                node = %destination,
                                "ignore outdated request timer",
                            );
                            return Ok(());
                        }
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

                        let source_route = SourceRoute::try_from(path)
                            .expect("Paths of vicinity graph include start of path");
                        Self::send_query_route_req(
                            context,
                            source_route,
                            req_state.expected_nonce,
                        )?;
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
                        req_state.timer_id = timeout_timer_id;
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
                    None => {} // timer of other use case
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

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::{
        Output,
        context::{
            ContextConfig,
            SyncContext,
        },
        domain::{
            ConnectionId,
            Path,
            routing_table::SingleBucketRT,
            underlay_neighbor_table::InMemoryULNTable,
            vicinity::vicinity_graph::PetVicinityGraph,
        },
        runtime::testing::TestingUseCaseRuntime,
        use_cases::{
            TimerId,
            UseCaseContext,
        },
    };

    const BUCKET_SIZE: usize = 20;
    type Ctx = SyncContext<
        SingleBucketRT<BUCKET_SIZE>,
        TestingUseCaseRuntime,
        (),
        InMemoryULNTable,
        PetVicinityGraph,
    >;
    type VD = VicinityDiscovery<Ctx, BUCKET_SIZE>;

    fn root() -> NodeId {
        NodeId::with_msb(1)
    }
    /// underlay neighbor
    fn uln() -> NodeId {
        NodeId::with_msb(2)
    }
    /// two-hop neighbor, only reachable via [uln]
    fn two_hop() -> NodeId {
        NodeId::with_msb(3)
    }
    fn ulnid() -> UnderlayNeighborId {
        UnderlayNeighborId {
            interface_id: InterfaceId::try_from(1).unwrap(),
            connection_id: ConnectionId(1),
        }
    }
    fn ssn(value: u32) -> SafeStateSeqNr {
        SafeStateSeqNr::try_from(value).unwrap()
    }

    /// Context with [uln] in the (unconfirmed) ULNTable and, if `uln_in_vicinity`,
    /// as direct neighbor in the vicinity graph, last seen at `t0`.
    fn setup(config: VicinityDiscoveryConfig, uln_in_vicinity: bool) -> (VD, Ctx, Instant) {
        crate::tests::init();

        let t0 = Instant::now();
        let runtime = TestingUseCaseRuntime::with_unique_timer_ids();
        runtime.set_current_time(t0);
        let ctx = Ctx::new(ContextConfig {
            root_id: root(),
            routing_table: SingleBucketRT::<BUCKET_SIZE>::new(root()),
            runtime,
            insertion_strategy: (),
            uln_table: InMemoryULNTable::new(),
            vicinity_graph: PetVicinityGraph::new(root()),
        });
        // unconfirmed, so no routing table contact is required when sending ULNDiscReqs
        ctx.uln_table_mut().insert(uln(), ulnid());
        if uln_in_vicinity {
            let mut vg = ctx.vicinity_graph_mut();
            vg.insert(uln(), &root(), ssn(1)).unwrap();
            vg.entry_mut(&uln()).unwrap().update_last_seen(t0);
        }

        let mut vd = VD::new(config);
        vd.start(&ctx).unwrap();
        (vd, ctx, t0)
    }

    fn make_uln_stale(ctx: &Ctx, t0: Instant) {
        let config = VicinityDiscoveryConfig::default();
        ctx.runtime()
            .set_current_time(t0 + config.uln_max_interval + Duration::from_secs(1));
    }

    fn fire(vd: &mut VD, ctx: &Ctx, timer_id: TimerId) {
        vd.handle_event(ctx, UseCaseEvent::Timer(timer_id)).unwrap();
    }

    fn stale_check_timer(vd: &VD) -> TimerId {
        let VDState::Running { timer_hooks, .. } = &vd.state else {
            panic!("not running");
        };
        timer_hooks
            .iter()
            .find_map(|(id, hook)| (hook == &TimerHook::TimeoutCheckForStaleULNs).then_some(*id))
            .expect("stale check timer registered")
    }

    fn pending_uln_disc_req(vd: &VD) -> Option<RequestState> {
        let VDState::Running { pending_reqs, .. } = &vd.state else {
            panic!("not running");
        };
        pending_reqs
            .get(&(uln(), ProtocolMessageKind::ULNDiscRsp))
            .cloned()
    }

    fn has_timer_hook(vd: &VD, timer_id: TimerId) -> bool {
        let VDState::Running { timer_hooks, .. } = &vd.state else {
            panic!("not running");
        };
        timer_hooks.contains_key(&timer_id)
    }

    /// Nonces of the ULNDiscReqs sent since the last call
    fn sent_uln_disc_reqs(ctx: &Ctx) -> Vec<Nonce> {
        ctx.runtime()
            .output()
            .filter_map(|output| match output {
                Output::SendProtocolMessage(msg @ ProtocolMessage::ULNDiscReq(_), destination) => {
                    assert_eq!(destination, UnderlayNeighbor(ulnid()));
                    Some(msg.msg_id().expect("ULNDiscReq has a nonce"))
                }
                _ => None,
            })
            .collect()
    }

    fn timer_duration(ctx: &Ctx, timer_id: TimerId) -> Duration {
        ctx.runtime()
            .registered_timers()
            .into_iter()
            .find_map(|(id, duration)| (id == timer_id).then_some(duration))
            .expect("timer registered")
    }

    /// starts a regular ULNDiscReq (as sent in reply to a ULNHello or for syncing)
    fn start_regular_uln_disc_req(vd: &mut VD, ctx: &Ctx) -> RequestState {
        let timeout = vd.config.uln_discovery_rsp_initial_max_wait_time;
        vd.init_uln_disc_req(ctx, uln(), ulnid(), timeout).unwrap();
        assert_eq!(sent_uln_disc_reqs(ctx).len(), 1);
        pending_uln_disc_req(vd).expect("request pending")
    }

    #[test]
    fn stale_check_runs_on_every_period() {
        let (mut vd, ctx, t0) = setup(VicinityDiscoveryConfig::default(), true);
        make_uln_stale(&ctx, t0);
        let timer = stale_check_timer(&vd);

        fire(&mut vd, &ctx, timer);
        assert_eq!(
            sent_uln_disc_reqs(&ctx).len(),
            1,
            "first check probes the stale ULN"
        );
        assert!(
            has_timer_hook(&vd, timer),
            "periodic timer hook must be kept"
        );

        // the runtime fires the periodic timer again under the same id
        fire(&mut vd, &ctx, timer);
        assert_eq!(
            sent_uln_disc_reqs(&ctx).len(),
            1,
            "second check probes again"
        );
        assert!(
            has_timer_hook(&vd, timer),
            "periodic timer hook must be kept"
        );
    }

    #[test]
    fn stale_check_does_not_probe_recently_seen_uln() {
        let (mut vd, ctx, _) = setup(VicinityDiscoveryConfig::default(), true);

        let timer = stale_check_timer(&vd);
        fire(&mut vd, &ctx, timer);
        assert!(sent_uln_disc_reqs(&ctx).is_empty());
        assert!(pending_uln_disc_req(&vd).is_none());
    }

    #[test]
    fn stale_check_starts_request_with_check_timeout() {
        let (mut vd, ctx, t0) = setup(VicinityDiscoveryConfig::default(), true);
        make_uln_stale(&ctx, t0);

        let timer = stale_check_timer(&vd);
        fire(&mut vd, &ctx, timer);
        let nonces = sent_uln_disc_reqs(&ctx);
        assert_eq!(nonces.len(), 1);

        let req = pending_uln_disc_req(&vd).expect("request pending");
        assert_eq!(req.expected_nonce, nonces[0]);
        assert_eq!(req.timeouts, 0);
        assert_eq!(req.timeout, vd.config.uln_discovery_rsp_check_max_wait_time);
    }

    #[test]
    fn stale_check_resends_pending_request_immediately() {
        let (mut vd, ctx, t0) = setup(VicinityDiscoveryConfig::default(), true);
        let before = start_regular_uln_disc_req(&mut vd, &ctx);
        make_uln_stale(&ctx, t0);

        let timer = stale_check_timer(&vd);
        fire(&mut vd, &ctx, timer);

        assert_eq!(
            sent_uln_disc_reqs(&ctx),
            vec![before.expected_nonce],
            "resent immediately with the nonce of the pending request"
        );
        let after = pending_uln_disc_req(&vd).expect("request still pending");
        assert_eq!(after.expected_nonce, before.expected_nonce);
        assert_eq!(after.timeouts, before.timeouts, "retry count is kept");
        let check_timeout = vd.config.uln_discovery_rsp_check_max_wait_time;
        assert_eq!(
            after.timeout, check_timeout,
            "switched to the shorter check timeout"
        );

        // the retry timer was replaced
        assert_ne!(after.timer_id, before.timer_id);
        assert_eq!(ctx.runtime().removed_timers(), vec![before.timer_id]);
        assert!(!has_timer_hook(&vd, before.timer_id));
        assert!(has_timer_hook(&vd, after.timer_id));
        assert!(timer_duration(&ctx, after.timer_id) <= check_timeout.mul_f64(1.5));
    }

    #[test]
    fn immediate_resend_keeps_shorter_pending_timeout() {
        let (mut vd, ctx, _) = setup(VicinityDiscoveryConfig::default(), true);
        let check_timeout = vd.config.uln_discovery_rsp_check_max_wait_time;
        vd.init_uln_disc_req(&ctx, uln(), ulnid(), check_timeout)
            .unwrap();

        let longer = vd.config.uln_discovery_rsp_initial_max_wait_time;
        vd.send_uln_disc_req_now(&ctx, uln(), ulnid(), longer)
            .unwrap();

        assert_eq!(pending_uln_disc_req(&vd).unwrap().timeout, check_timeout);
    }

    #[test]
    fn outdated_request_timer_is_ignored() {
        let (mut vd, ctx, t0) = setup(VicinityDiscoveryConfig::default(), true);
        let before = start_regular_uln_disc_req(&mut vd, &ctx);
        make_uln_stale(&ctx, t0);
        let timer = stale_check_timer(&vd);
        fire(&mut vd, &ctx, timer);
        sent_uln_disc_reqs(&ctx);
        let replaced = pending_uln_disc_req(&vd).unwrap();

        // the old timer still fires (e.g. if the runtime could not remove it in time)
        vd.state = match vd.state {
            VDState::Running {
                resync_timer_id,
                mut timer_hooks,
                pending_reqs,
                interfaces,
            } => {
                timer_hooks.insert(before.timer_id, TimerHook::TimeoutULNDiscReq(uln()));
                VDState::Running {
                    resync_timer_id,
                    timer_hooks,
                    pending_reqs,
                    interfaces,
                }
            }
            state => state,
        };
        fire(&mut vd, &ctx, before.timer_id);
        assert!(
            sent_uln_disc_reqs(&ctx).is_empty(),
            "outdated timer must not resend"
        );
        assert_eq!(
            pending_uln_disc_req(&vd),
            Some(replaced.clone()),
            "request unchanged"
        );

        // the current timer still works
        fire(&mut vd, &ctx, replaced.timer_id);
        assert_eq!(sent_uln_disc_reqs(&ctx), vec![before.expected_nonce]);
        assert_eq!(pending_uln_disc_req(&vd).unwrap().timeouts, 1);
    }

    #[test]
    fn stale_uln_is_removed_after_max_retries_despite_immediate_resend() {
        let (mut vd, ctx, t0) = setup(VicinityDiscoveryConfig::default(), true);
        start_regular_uln_disc_req(&mut vd, &ctx);
        // use up the retries of the pending request
        for _ in 0..vd.config.uln_discovery_max_retries {
            let timer = pending_uln_disc_req(&vd).unwrap().timer_id;
            fire(&mut vd, &ctx, timer);
        }
        sent_uln_disc_reqs(&ctx);

        make_uln_stale(&ctx, t0);
        let timer = stale_check_timer(&vd);
        fire(&mut vd, &ctx, timer);
        let req = pending_uln_disc_req(&vd).unwrap();
        assert_eq!(
            req.timeouts, vd.config.uln_discovery_max_retries,
            "retry count is kept"
        );

        fire(&mut vd, &ctx, req.timer_id);
        assert!(pending_uln_disc_req(&vd).is_none(), "request given up");
        assert!(!ctx.uln_table().contains(&uln()), "ULN removed");
        assert!(
            ctx.vicinity_graph().entry(&uln()).is_none(),
            "ULN removed from vicinity"
        );
    }

    #[test]
    fn sync_timeout_removes_nodes_no_longer_reachable() {
        let (mut vd, ctx, _) = setup(VicinityDiscoveryConfig::default(), true);
        ctx.vicinity_graph_mut()
            .insert(two_hop(), &uln(), ssn(1))
            .unwrap();

        start_regular_uln_disc_req(&mut vd, &ctx);
        for _ in 0..=vd.config.uln_discovery_max_retries {
            let timer = pending_uln_disc_req(&vd).unwrap().timer_id;
            fire(&mut vd, &ctx, timer);
        }

        assert!(pending_uln_disc_req(&vd).is_none(), "request given up");
        assert!(!ctx.uln_table().contains(&uln()), "ULN removed");
        let vg = ctx.vicinity_graph();
        // the ULN keeps its edge to the two-hop node, but neither is reachable anymore
        assert!(
            vg.entry(&uln()).is_none(),
            "unreachable ULN removed from vicinity"
        );
        assert!(
            vg.entry(&two_hop()).is_none(),
            "unreachable two-hop node removed from vicinity"
        );
        drop(vg);
        assert!(
            ctx.runtime()
                .broadcast()
                .any(|event| matches!(event, UseCaseEvent::Vicinity(VicinityEvent::Changed))),
            "vicinity change is announced"
        );
    }

    /// another underlay neighbor, not affected by the loss of [uln]
    fn other_uln() -> NodeId {
        NodeId::with_msb(4)
    }
    /// two-hop neighbor reachable via [other_uln]
    fn behind_other_uln() -> NodeId {
        NodeId::with_msb(5)
    }

    fn add_contact(ctx: &Ctx, path: &[NodeId]) {
        ctx.routing_table_mut()
            .add(Contact::new(Path::try_from(path).unwrap(), ssn(1)))
            .unwrap();
    }

    fn give_up_regular_uln_disc_req(vd: &mut VD, ctx: &Ctx) {
        start_regular_uln_disc_req(vd, ctx);
        for _ in 0..=vd.config.uln_discovery_max_retries {
            let timer = pending_uln_disc_req(vd).unwrap().timer_id;
            fire(vd, ctx, timer);
        }
        assert!(pending_uln_disc_req(vd).is_none(), "request given up");
    }

    #[test]
    fn sync_timeout_invalidates_contacts_via_lost_uln() {
        let (mut vd, ctx, _) = setup(VicinityDiscoveryConfig::default(), true);
        ctx.vicinity_graph_mut()
            .insert(two_hop(), &uln(), ssn(1))
            .unwrap();
        add_contact(&ctx, &[uln()]);
        add_contact(&ctx, &[uln(), two_hop()]);
        add_contact(&ctx, &[other_uln()]);
        add_contact(&ctx, &[other_uln(), behind_other_uln()]);

        give_up_regular_uln_disc_req(&mut vd, &ctx);

        let rt = ctx.routing_table();
        // the ULN left the vicinity, so its contact is removed
        assert!(!rt.contains(&uln()), "contact of lost ULN removed");
        let via_lost = rt.contact(&two_hop()).unwrap();
        assert!(via_lost.is_invalid(), "contact via lost ULN invalidated");
        for node in [other_uln(), behind_other_uln()] {
            assert!(
                rt.contact(&node).unwrap().is_valid(),
                "contact {node} stays valid"
            );
        }
        assert!(
            rt.iter()
                .filter(|contact| contact.is_valid())
                .filter_map(Contact::path)
                .all(|path| path.first() != &uln()),
            "no valid contact starts with the lost ULN"
        );
    }

    #[test]
    fn sync_timeout_invalidates_contact_of_uln_still_in_vicinity() {
        let (mut vd, ctx, _) = setup(VicinityDiscoveryConfig::default(), false);
        {
            // the ULN is also a two-hop neighbor via another ULN
            let mut vg = ctx.vicinity_graph_mut();
            vg.insert(other_uln(), &root(), ssn(1)).unwrap();
            vg.insert(uln(), &other_uln(), ssn(1)).unwrap();
        }
        add_contact(&ctx, &[uln()]);
        add_contact(&ctx, &[uln(), two_hop()]);

        give_up_regular_uln_disc_req(&mut vd, &ctx);

        assert!(
            ctx.vicinity_graph().entry(&uln()).is_some(),
            "ULN still in vicinity"
        );
        let rt = ctx.routing_table();
        assert!(
            rt.contact(&uln()).unwrap().is_invalid(),
            "direct contact of ULN invalidated"
        );
        assert!(
            rt.contact(&two_hop()).unwrap().is_invalid(),
            "contact via ULN invalidated"
        );
    }

    #[test]
    fn uln_hello_resends_pending_request_immediately() {
        // always answer ULNHellos regardless of the deterministic heuristic
        let config = VicinityDiscoveryConfig {
            heuristic_max_wait_time: Duration::ZERO,
            ..Default::default()
        };
        // not in the vicinity graph yet, so the ULNHello is from a new neighbor
        let (mut vd, ctx, _) = setup(config, false);
        vd.handle_event(
            &ctx,
            UseCaseEvent::UnderlayUpdate(UnderlayNeighborUpdate::InterfaceUp(ulnid().interface_id)),
        )
        .unwrap();
        let before = start_regular_uln_disc_req(&mut vd, &ctx);

        let hello = ProtocolMessage::ULNHello(CommonHeader::new(
            ProtocolMessageKind::ULNHello,
            uln(),
            NodeId::ALL_NODES,
            None,
            StateSeqNr::Value(ssn(1)),
            0,
        ));
        vd.handle_event(
            &ctx,
            UseCaseEvent::Message(hello, UnderlayNeighborSource::UnderlayNeighbor(ulnid())),
        )
        .unwrap();

        assert_eq!(sent_uln_disc_reqs(&ctx), vec![before.expected_nonce]);
        let after = pending_uln_disc_req(&vd).unwrap();
        assert_eq!(after.expected_nonce, before.expected_nonce);
        assert_ne!(after.timer_id, before.timer_id);
        assert_eq!(ctx.runtime().removed_timers(), vec![before.timer_id]);
    }

    #[test]
    fn deterministic_handshake_heuristic() {
        let mut test_pairs = vec![
            (NodeId::from_const(0xe3456789u128), NodeId::from_const(0xf3456788u128)),
            (NodeId::from_const(0x2e3456789u128), NodeId::from_const(0x1f3456788u128)),
            (NodeId::from_const(0xf2456789u128), NodeId::from_const(0xf1456788u128)),
            (NodeId::from_const(0xf1456789u128), NodeId::from_const(0xf2456789u128)),
            (NodeId::from_const(0x80000000u128), NodeId::from_const(0x80000001u128)),
            (NodeId::from_const(0x80000001u128), NodeId::from_const(0x80000000u128)),
            (NodeId::from_const(0x100000000u128), NodeId::from_const(0x200000000u128)),
            (NodeId::from_const(0x200000000u128), NodeId::from_const(0x100000000u128)),
            (NodeId::from_const(0xd6af3c5d709daef70f9b0ee6ae67u128),NodeId::from_const(0xff305a2c3b78cc824318bb16c474u128)),
        ];

        for _i in 0..64 {
          test_pairs.push((NodeId::random(), NodeId::random()));
        };
        for (node_id_a, node_id_b) in test_pairs {
            assert_ne!(deterministic_heuristic(&node_id_a, &node_id_b, NonZeroU8::new(32).unwrap()),
                       deterministic_heuristic(&node_id_b, &node_id_a, NonZeroU8::new(32).unwrap()),"{node_id_a:x},{node_id_b:x}");

        }
    }
}
