use std::collections::HashMap;
use std::marker::PhantomData;
use std::net::Ipv6Addr;
use std::num::NonZeroUsize;
use std::ops::Deref;
use std::time::Duration;

use derive_more::derive::{Display, Error};
use rand::Rng;

use crate::domain::UnderlayNeighborDestination::{Broadcast, BroadcastInterface, UnderlayNeighbor};
use crate::domain::{
    Contact, ContactState, DEFAULT_BUCKET_SIZE, InterfaceId, NodeId, Path, RoutingTable,
    StateSeqNr, UNTable, UnderlayNeighborId, UnderlayNeighborUpdate, node_id,
};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{
    HelloMessage, Nonce, ProtocolMessage, QueryRouteReqData, QueryRouteType, RTableData,
    ReqRspMessage,
};
use crate::use_cases::{
    BroadcastableUseCaseEvent, ContactEvent, EventHandler, TimerId, UseCase, UseCaseContext,
    UseCaseEvent, UseCaseRuntime, UseCaseState,
};

/// Radius of the neighborhood considered as vicinity.
///
/// A radius of **3** means:
///
/// - Contacts with a distance **<= 3** hops (*path length <= 4*) are in the vicinity.
/// - Contacts with a distance **< 3** hops (*path length < 4*) receive QueryRouteReqs.
///     Except the underlay neighbors with a distance of 0 hops (*path length == 1*) with
///     which the following messages are exchanged: *PNHello, PNDiscReq, PNDiscRsp*.
pub const VICINITY_RADIUS: usize = 2;

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub struct VicinityDiscoveryConfig {
    pub max_timeout: Duration,
    /// Initial timeout duration to use for sending PNHello messages.
    ///
    /// If this is bigger than `probing_timeout` this will only be used once on initialization
    /// and after that the `probing_timeout` will be used to calculate the timers duration.
    pub initial_timeout: Duration,
    /// Random scatter to not send PNHello messages in a fixed interval.
    pub max_scatter: Duration,
    /// Number of bits for the deterministic heuristic to consider for deciding which node should
    /// respond to the PNHello message.
    pub heuristic_calculation_bits: NonZeroUsize,
    /// Turns of the heuristic and nodes in vicinity will always respond to PNHello messages.
    pub heuristic_enabled: bool,
    /// Timeout duration to use for processing the queue of nodes that need to be resynchronised.
    pub resync_timeout: Duration,
    /// Maximum number of nodes that are queried that need to be resynchronized during one
    /// resynchronisation phase.
    pub resynch_count: usize,
    /// Maximum number of resynchronisation attempts after which the node is deleteted from the
    /// resynchronisation queue.
    pub max_resync_tries: usize,
}

impl Default for VicinityDiscoveryConfig {
    fn default() -> Self {
        Self {
            max_timeout: Duration::from_secs(30),
            initial_timeout: Duration::from_millis(250),
            max_scatter: Duration::from_millis(225),
            heuristic_calculation_bits: NonZeroUsize::new(32).unwrap(),
            heuristic_enabled: true,
            resync_timeout: Duration::from_millis(100),
            resynch_count: 5,
            max_resync_tries: 5,
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

#[derive(Debug, Eq, PartialEq, Clone, Default)]
pub enum VDState {
    #[default]
    Initialized,
    Running {
        last_timeout: Duration,
        hello_timer_id: TimerId,
        last_ssn: StateSeqNr,
        resync_timer_id: TimerId,
        resync_queue: HashMap<NodeId, (StateSeqNr, usize)>,
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

impl<C> Default for VicinityDiscovery<C> {
    fn default() -> Self {
        Self::new(VicinityDiscoveryConfig::default())
    }
}

/// Returns if the node should answer to the other nodes PNHello.
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
            let mut thread_rng = rand::thread_rng();

            let min_timeout = self.config.resync_timeout.mul_f32(0.5);
            let max_timout = self.config.resync_timeout.mul_f32(1.5);

            let random_timeout = thread_rng.gen_range(min_timeout..=max_timout);
            *resync_timer_id = context.runtime_mut().register_timer(random_timeout);
        }
    }
}

impl<C, const BUCKET_SIZE: usize> VicinityDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::PhysicalNeighborTable: UNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    fn send_query_route_req(&mut self, context: &C, contact: Contact) -> Result<(), VDError> {
        Self::restricted_send_query_route_req(context, contact)
    }

    fn restricted_send_query_route_req(context: &C, contact: Contact) -> Result<(), VDError> {
        // Physical Neighbors and Nodes outside of the Vicinity are not included
        if contact.is_pn() || contact.path().size() > VICINITY_RADIUS {
            log::trace!(target: "vicinity_discovery", "Ignoring contact update: underlay neighbor or not in vicinity radius");
            return Ok(());
        }

        // Only Valid Contacts are considered
        if contact.state() != &ContactState::Valid {
            log::trace!(target: "vicinity_discovery", "Ignoring contact update: contact not valid");
            return Ok(());
        }

        // Convert contacts path to source route
        let mut route = SourceRoute::from(contact.path().clone());
        route.push_front(*context.root_id());

        // Get interface of route
        let neighbor_port = context.pn_table().get(contact.path().first()).cloned();
        if neighbor_port.is_none() {
            log::error!(
                target: "vicinity_discovery",
                "Temporary inconsistency: Valid contacts path starts with invalid underlay neighbor {}",
                contact.path().first()
            );
            return Err(VDError::NeighborInconsistency);
        }

        // Request only underlay Neighborhood of that Node
        let request = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
            data: QueryRouteReqData {
                query_type: QueryRouteType::PhysicalNeighbors,
            },
            not_via: context.not_via().clone(),
            source_route: route,
        };

        log::trace!(target: "vicinity_discovery", "Sending message {:?}", request);

        context
            .runtime_mut()
            .send_message(request, context.pn_table().deref());

        Ok(())
    }

    fn send_query_route_rsp(
        &self,
        context: &C,
        request: ReqRspMessage<QueryRouteReqData>,
    ) -> Result<(), VDError> {
        let contacts = match request.data.query_type {
            QueryRouteType::PhysicalNeighbors => {
                let pn_lock = context.pn_table();
                let rt_lock = context.routing_table();

                let result: Vec<Contact> = pn_lock
                    .keys()
                    .map(|id| {
                        (id, rt_lock
                            .contact(id)
                            .cloned()
                            .ok_or(VDError::NeighborInconsistency))
                    })
                    .filter_map(|(id, result)| match result {
                        Ok(contact) => Some(contact),
                        Err(_) => {
                            log::warn!(target: "vicinity_discovery", "No contact for pn {} found", id);
                            None
                        }
                    })
                    .collect();
                result
            }
        };

        let message = ProtocolMessage::QueryRouteRsp(ReqRspMessage {
            nonce: request.nonce,
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
            data: RTableData { contacts },
            not_via: context.not_via().clone(),
            source_route: SourceRoute::from_reversed(request.source_route),
        });

        log::trace!(target: "vicinity_discovery", "Sending: {:?}", message);

        context
            .runtime_mut()
            .send_message(message, context.pn_table().deref());

        Ok(())
    }

    fn constract_hello(&self, context: &C) -> HelloMessage {
        HelloMessage {
            source: *context.root_id(),
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
        }
    }

    fn broadcast_hello(&self, context: &C) {
        let message = self.constract_hello(context);
        let source_ip = Ipv6Addr::from(context.root_id()).to_string();
        log::trace!(target: "vicinity_discovery", "Sending message {:?} from {:?}", message, source_ip);
        context.runtime_mut().send_message_via(message, Broadcast);
    }

    fn broadcast_interface_hello(&self, context: &C, interface: InterfaceId) {
        let message = self.constract_hello(context);
        let source_ip = Ipv6Addr::from(context.root_id()).to_string();
        log::trace!(target: "vicinity_discovery", "Sending message {:?} from {:?}", message, source_ip);
        context
            .runtime_mut()
            .send_message_via(message, BroadcastInterface(interface));
    }

    fn send_hello(&self, context: &C, ulnid: UnderlayNeighborId) {
        let message = self.constract_hello(context);
        let source_ip = Ipv6Addr::from(context.root_id()).to_string();
        log::trace!(target: "vicinity_discovery", "Sending message {:?} from {:?}", message, source_ip);
        context
            .runtime_mut()
            .send_message_via(message, UnderlayNeighbor(ulnid));
    }

    fn send_pn_disc_req(&self, context: &C, source: NodeId) {
        Self::restricted_send_pn_disc_req(context, source)
    }
    fn restricted_send_pn_disc_req(context: &C, source: NodeId) {
        // Answer with a PNDiscReq to ensure bidirectional connectivity
        let pn_contacts = context
            .pn_table()
            .iter()
            .filter_map(|(id, _)| context.routing_table().contact(id).cloned())
            .collect::<Vec<_>>();

        let message = ProtocolMessage::PNDiscReq(ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
            data: RTableData {
                contacts: pn_contacts,
            },
            not_via: context.not_via().clone(),
            // Source route is ignored, as only underlay neighbors get these
            source_route: SourceRoute::from(Path::from([*context.root_id(), source])),
        });

        log::trace!(
            target: "vicinity_discovery",
            "Sending message: {:?}",
            message
        );

        context
            .runtime_mut()
            .send_message(message, context.pn_table().deref());
    }

    fn send_pn_disc_rsp(&self, context: &C, req: ReqRspMessage<RTableData>) -> Result<(), VDError> {
        // Note: Locks will be released at end of curly braces
        let (ssn, contacts) = {
            let rt_lock = context.routing_table();
            let pn_lock = context.pn_table();

            let neighbors = pn_lock.keys().collect::<Vec<_>>();
            let mut contacts = Vec::with_capacity(neighbors.len());
            for pn_id in neighbors {
                let contact = rt_lock.contact(pn_id).cloned();
                if contact.is_none() {
                    return Err(VDError::NeighborInconsistency);
                }
                contacts.push(contact.unwrap());
            }
            (*pn_lock.state_seq_nr(), contacts)
        };

        let response = ProtocolMessage::PNDiscRsp(ReqRspMessage {
            nonce: req.nonce,
            source_state_seq_nr: ssn,
            data: RTableData { contacts },
            not_via: context.not_via().clone(),
            source_route: SourceRoute::from_reversed(req.source_route),
        });

        log::trace!(target: "vicinity_discovery", "Sending: {:?}", response);

        context
            .runtime_mut()
            .send_message(response, context.pn_table().deref());

        Ok(())
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase for VicinityDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::PhysicalNeighborTable: UNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    type State = VDState;

    fn start(&mut self, context: &C) -> Result<(), Self::Error> {
        let hello_timer_id = context
            .runtime_mut()
            .register_timer(self.config.initial_timeout);

        let mut thread_rng = rand::thread_rng();
        let min_timeout = self.config.resync_timeout.mul_f32(0.5);
        let max_timout = self.config.resync_timeout.mul_f32(1.5);
        let random_timeout = thread_rng.gen_range(min_timeout..=max_timout);
        let resync_timer_id = context.runtime_mut().register_timer(random_timeout);

        self.state = VDState::Running {
            last_timeout: self.config.initial_timeout,
            hello_timer_id,
            last_ssn: *context.pn_table().state_seq_nr(),
            resync_queue: HashMap::default(),
            resync_timer_id,
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
    C::PhysicalNeighborTable: UNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    type Context = C;
    type Error = VDError;
    type Value = ();

    fn handle_event(&mut self, context: &C, event: UseCaseEvent) -> Result<(), Self::Error> {
        match (event.clone(), &mut self.state) {
            // ========== Vicinity Discovery - Query Route ==========
            (
                UseCaseEvent::Contact(ContactEvent::New(contact)),
                VDState::Running { resync_queue, .. },
            ) => {
                let _resync_queue = resync_queue;
                //if contact.path().size() <= VICINITY_RADIUS {
                //    resync_queue.insert(*contact.id(), (contact.state_seq_nr().clone(), 0));
                //}

                self.send_query_route_req(context, contact)?;
            }
            (UseCaseEvent::Contact(ContactEvent::Updated { new, old }), _) => {
                // Only if path changed. Path length checks and everything else is done in discover
                if new.state_seq_nr() > old.state_seq_nr() || new.path() != old.path() {
                    self.send_query_route_req(context, new)?;
                }
            }
            (UseCaseEvent::Message(ProtocolMessage::QueryRouteReq(request), _), _) => {
                if request.destination() != context.root_id() {
                    return Ok(());
                }

                self.send_query_route_rsp(context, request)?;
            }
            // ========== Physical Neighbor Discovery ==========
            (
                UseCaseEvent::Timer(id),
                VDState::Running {
                    last_timeout,
                    hello_timer_id,
                    last_ssn,
                    ..
                },
            ) if &id == hello_timer_id => {
                let current_ssn = *context.pn_table().state_seq_nr();

                // reset to initial timeout if ssn changed
                let next_timeout = if *last_ssn != current_ssn {
                    self.config.initial_timeout
                } else {
                    let next_timeout_increased = 2 * *last_timeout;
                    let new_duration = if next_timeout_increased < self.config.max_timeout {
                        next_timeout_increased
                    } else {
                        self.config.max_timeout
                    };

                    // Randomize in a given scatter interval
                    let mut thread_rng = rand::thread_rng();
                    let random_scatter = if self.config.max_scatter.is_zero() {
                        Duration::ZERO
                    } else {
                        thread_rng.gen_range(Duration::from_millis(0)..self.config.max_scatter)
                    };
                    new_duration - self.config.max_scatter / 2 + random_scatter
                };
                let next_hello_timer_id = context.runtime_mut().register_timer(next_timeout);

                *last_timeout = next_timeout;
                *hello_timer_id = next_hello_timer_id;
                *last_ssn = current_ssn;

                self.broadcast_hello(context);
            }
            (
                UseCaseEvent::Message(
                    ProtocolMessage::Hello(HelloMessage {
                        source,
                        source_state_seq_nr,
                    }),
                    _,
                ),
                _,
            ) => {
                if self.config.heuristic_enabled
                    && !deterministic_heuristic(
                        context.root_id(),
                        &source,
                        self.config.heuristic_calculation_bits,
                    )
                {
                    log::trace!(target: "vicinity_discovery", "Not responding to Hello from {}", source);
                    return Ok(());
                }

                // we already know the neighbor
                // only resynchronise if we see a newer ssn in the hello message
                if context.pn_table().contains(&source) {
                    if let VDState::Running { resync_queue, .. } = &self.state {
                        if let Some((expected_ssn, _)) = resync_queue.get(&source) {
                            // nothing new about the neighbor
                            if &source_state_seq_nr < expected_ssn {
                                log::trace!(target: "vicinity_discovery", "Not responding to Hello from unchanged underlay neighbor {} as we received an unexpected state sequence number", source);
                                return Ok(());
                            }
                        }
                    } else {
                        // check rt contact for expected ssn
                        let rt = context.routing_table();
                        let Some(contact) = rt.contact(&source) else {
                            return Err(VDError::NeighborInconsistency);
                        };

                        if &source_state_seq_nr <= contact.state_seq_nr() {
                            log::trace!(target: "vicinity_discovery", "Not responding to Hello from unchanged underlay neighbor {}", source);
                            return Ok(());
                        }
                    }
                }

                self.send_pn_disc_req(context, source);
            }
            (UseCaseEvent::Message(ProtocolMessage::PNDiscReq(req), _), _) => {
                if req.destination() != context.root_id() {
                    return Ok(());
                }
                self.send_pn_disc_rsp(context, req)?;
            }
            (
                UseCaseEvent::UnderlayUpdate(UnderlayNeighborUpdate::UnderlayNeighborUp(ulnid)),
                _,
            ) => {
                // send hello message immediately if interface comes up
                // TODO: add small random delay
                // TODO: reset hello timer if this fires too far away in the future
                // TODO: send hello to up neighbor only
                self.send_hello(context, ulnid);
            }
            (UseCaseEvent::UnderlayUpdate(UnderlayNeighborUpdate::InterfaceUp(interface)), _) => {
                self.broadcast_interface_hello(context, interface);
            }
            // reacting on InterfaceDown is done in the FailureHandling use-case
            // ========== Resynchronisation management ==========
            (
                UseCaseEvent::ResyncNode(node_id, expected_ssn),
                VDState::Running { resync_queue, .. },
            ) => {
                // only update expected_ssn, if greater
                if let Some((previous_expected_ssn, _)) = resync_queue.get_mut(&node_id) {
                    if *previous_expected_ssn < expected_ssn {
                        log::trace!(target: "vicinity_discovery", "Update expected state sequence number of node {}: {}", node_id, expected_ssn);
                        *previous_expected_ssn = expected_ssn;
                    }
                } else {
                    log::trace!(target: "vicinity_discovery", "Add node to resynchronisation queue: {}", node_id);
                    resync_queue.insert(node_id, (expected_ssn, 0));
                }
            }
            (
                UseCaseEvent::Timer(timer_id),
                VDState::Running {
                    resync_timer_id,
                    resync_queue,
                    ..
                },
            ) if &timer_id == resync_timer_id => {
                let mut max_retries_reached =
                    Vec::with_capacity(std::cmp::max(self.config.resynch_count, 10));

                // TODO refresh nodes in the lowest bucket first
                for (nid, (_, current_tries)) in
                    resync_queue.iter_mut().take(self.config.resynch_count)
                {
                    if context.pn_table().contains(nid) {
                        Self::restricted_send_pn_disc_req(context, *nid);
                    } else if let Some(contact) = context.routing_table().contact(nid) {
                        Self::restricted_send_query_route_req(context, contact.clone())?;
                    }

                    *current_tries += 1;
                    if *current_tries >= self.config.max_resync_tries {
                        max_retries_reached.push(*nid);
                    }
                }

                // FIXME this needs to be synced somehow to precompute_paths_and_path_ids
                for nid_max_retries_reached in max_retries_reached {
                    resync_queue.remove(&nid_max_retries_reached);
                }

                self.set_next_resync_timeout(context);
            }

            _ => {}
        }

        // stopping resynchronisation process on receiving expected ssn
        match (event, &mut self.state) {
            (
                UseCaseEvent::Message(ProtocolMessage::PNDiscReq(payload), _),
                VDState::Running { resync_queue, .. },
            )
            | (
                UseCaseEvent::Message(ProtocolMessage::PNDiscRsp(payload), _),
                VDState::Running { resync_queue, .. },
            )
            | (
                UseCaseEvent::Message(ProtocolMessage::QueryRouteRsp(payload), _),
                VDState::Running { resync_queue, .. },
            ) => {
                let source_ssn = &payload.source_state_seq_nr;
                let source_id = *payload.source();

                // TODO figure out if this actually works with a cloned source_id
                if let std::collections::hash_map::Entry::Occupied(entry) =
                    resync_queue.entry(source_id)
                {
                    let (expected_ssn, _) = entry.get();
                    if source_ssn >= expected_ssn {
                        entry.remove();
                        log::debug!(target: "vicinity_discovery", "No longer trying to resynchronise with node {}", source_id);
                    } else {
                        log::trace!(target: "vicinity_discovery", "Not received expected state sequence number ({}) of node {}: {}", expected_ssn, source_id, source_ssn);
                    }
                } else {
                    log::trace!(target: "vicinity_discovery", "Not expecting any state sequence number from {}", source_id);
                }
            }
            _ => {}
        }
        Ok(())
    }
}
