use std::collections::HashMap;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::marker::PhantomData;
use std::net::Ipv6Addr;
use std::num::NonZeroUsize;
use std::ops::Deref;
use std::time::Duration;

use rand::Rng;
use tokio::runtime::Runtime;

use crate::context::UseCaseContext;
use crate::domain::{
    node_id, Contact, ContactState, NetworkInterface, NodeId, PNTable, Path, RoutingTable,
    StateSeqNr, DEFAULT_BUCKET_SIZE,
};
use crate::hardware_events::HardwareEvent;
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{
    HelloMessage, Nonce, ProtocolMessage, ProtocolMessageSender, QueryRouteReqData, QueryRouteType,
    RTableData, ReqRspMessage,
};
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{ContactEvent, EventHandler, TimerId, UseCase, UseCaseEvent, UseCaseState};
use crate::utils::sync::Sender;
use crate::Output;

/// Radius of the neighborhood considered as vicinity.
///
/// A radius of **3** means:
///
/// - Contacts with a distance **<= 3** hops (*path length <= 4*) are in the vicinity.
/// - Contacts with a distance **< 3** hops (*path length < 4*) receive QueryRouteReqs.
///     Except the physical neighbors with a distance of 0 hops (*path length == 1*) with
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
#[derive(Debug, Eq, PartialEq, Clone)]
pub enum VDError {
    /// Sending a ProtocolMessage failed.
    MessageSendFailed,
    /// A Contact contains an invalid neighbor.
    NeighborInconsistency,
}

impl Display for VDError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MessageSendFailed => write!(
                f,
                "Sending a ProtocolMessage through a MessageSender failed"
            ),
            Self::NeighborInconsistency => write!(f, "Contact contains invalid neighbor"),
        }
    }
}

impl Error for VDError {}

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
/// Handles the discovery of the physical neighborhood (*vicinity*) including exchanging messages
/// with physical neighbors.
///
/// If a new [Contact] was added to the [RoutingTable] or an existing one was updated and
/// has a physical distance in the range of [1, [VICINITY_RADIUS]] hops (*path length is in
/// [2, [VICINITY_RADIUS] + 1]) a QueryRouteReq is sent to them to get their physical neighbors.
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
            panic!("Number of bits to use for the heuristic in VicinityDiscovery is greater than BIT_SIZE of NodeId.")
        }
        Self {
            _c: PhantomData::default(),
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
            *resync_timer_id = context.runtime().register_timer(random_timeout);
        }
    }
}

impl<C, const BUCKET_SIZE: usize> VicinityDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::MessageSender: ProtocolMessageSender,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::PhysicalNeighborTable: PNTable + Deref<Target = HashMap<NodeId, NetworkInterface>>,
{
    fn send_query_route_req(&mut self, context: &C, contact: Contact) -> Result<(), VDError> {
        Self::restricted_send_query_route_req(context, contact)
    }

    fn restricted_send_query_route_req(context: &C, contact: Contact) -> Result<(), VDError> {
        // Physical Neighbors and Nodes outside of the Vicinity are not included
        if contact.is_pn() || contact.path().size() > VICINITY_RADIUS {
            log::trace!(target: "vicinity_discovery", "Ignoring contact update: physical neighbor or not in vicinity radius");
            return Ok(());
        }

        // Only Valid Contacts are considered
        if contact.state() != &ContactState::Valid {
            log::trace!(target: "vicinity_discovery", "Ignoring contact update: contact not valid");
            return Ok(());
        }

        // Convert contacts path to source route
        let mut route = SourceRoute::from(contact.path().clone());
        route.push_front(context.root_id().clone());

        // Get interface of route
        let neighbor_port = context.pn_table().get(contact.path().first()).cloned();
        if neighbor_port.is_none() {
            log::error!(
                target: "vicinity_discovery",
                "Temporary inconsistency: Valid contacts path starts with invalid physical neighbor {}",
                contact.path().first()
            );
            return Err(VDError::NeighborInconsistency);
        }

        // Request only physical Neighborhood of that Node
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

        if let Err(e) = context.message_sender_mut().send_message(request) {
            log::error!(target: "vicinity_discovery", "Failed to send QueryRouteReq: {:?}", e);
            return Err(VDError::MessageSendFailed);
        }

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

        if let Err(e) = context.message_sender_mut().send_message(message) {
            log::error!(target: "vicinity_discovery", "Failed to send: {}", e);
            return Err(VDError::MessageSendFailed);
        }

        Ok(())
    }

    fn send_hello(&self, context: &C) -> Result<(), VDError> {
        let message = HelloMessage {
            source: context.root_id().clone(),
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
        };
        let source_ip = Ipv6Addr::from(context.root_id()).to_string();
        log::trace!(target: "vicinity_discovery", "Sending message {:?} from {:?}", message, source_ip);
        if let Err(e) = context.message_sender_mut().send_message(message) {
            log::error!(target: "vicinity_discovery",
                "MessageSender failed: {}",
                e
            );
            return Err(VDError::MessageSendFailed);
        }

        Ok(())
    }

    fn send_pn_disc_req(&self, context: &C, source: NodeId) -> Result<(), VDError> {
        Self::restricted_send_pn_disc_req(context, source)
    }
    fn restricted_send_pn_disc_req(context: &C, source: NodeId) -> Result<(), VDError> {
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
            // Source route is ignored, as only physical neighbors get these
            source_route: SourceRoute::from(Path::from([context.root_id().clone(), source])),
        });

        log::trace!(
            target: "vicinity_discovery",
            "Sending message: {:?}",
            message
        );

        if let Err(e) = context.message_sender_mut().send_message(message) {
            log::error!("Failed to send message: {}", e);
            return Err(VDError::MessageSendFailed);
        }

        Ok(())
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

        if let Err(e) = context.message_sender_mut().send_message(response) {
            log::error!(target: "vicinity_discovery", "Failed to send PNDiscRsp: {}", e);
            return Err(VDError::MessageSendFailed);
        }

        Ok(())
    }
}

impl<C, const BUCKET_SIZE: usize, S> UseCase for VicinityDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext<Runtime = UseCaseRuntime<S>>,
    S: Sender<Output>,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::PhysicalNeighborTable: PNTable + Deref<Target = HashMap<NodeId, NetworkInterface>>,
{
    type State = VDState;

    fn start(&mut self, context: &Self::Context) -> Result<(), Self::Error> {
        let hello_timer_id = context
            .runtime()
            .register_timer(self.config.initial_timeout);

        let mut thread_rng = rand::thread_rng();
        let min_timeout = self.config.resync_timeout.mul_f32(0.5);
        let max_timout = self.config.resync_timeout.mul_f32(1.5);
        let random_timeout = thread_rng.gen_range(min_timeout..=max_timout);
        let resync_timer_id = context.runtime().register_timer(random_timeout);

        self.state = VDState::Running {
            last_timeout: self.config.initial_timeout,
            hello_timer_id,
            last_ssn: context.pn_table().state_seq_nr().clone(),
            resync_queue: HashMap::default(),
            resync_timer_id,
        };

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

impl<C, const BUCKET_SIZE: usize, S> EventHandler for VicinityDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext<Runtime = UseCaseRuntime<S>>,
    C::MessageSender: ProtocolMessageSender,
    S: Sender<Output>,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::PhysicalNeighborTable: PNTable + Deref<Target = HashMap<NodeId, NetworkInterface>>,
{
    type Context = C;
    type Error = VDError;
    type Value = ();

    fn handle_event(
        &mut self,
        context: &Self::Context,
        event: UseCaseEvent,
    ) -> Result<(), Self::Error> {
        match (event.clone(), &mut self.state) {
            // ========== Vicinity Discovery - Query Route ==========
            (
                UseCaseEvent::Contact(ContactEvent::New(contact)),
                VDState::Running { resync_queue, .. },
            ) => {
                //if contact.path().size() <= VICINITY_RADIUS {
                //    resync_queue.insert(contact.id().clone(), (contact.state_seq_nr().clone(), 0));
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
                let current_ssn = context.pn_table().state_seq_nr().clone();

                // reset to initial timeout if ssn changed
                let next_timeout = if *last_ssn != current_ssn {
                    self.config.initial_timeout
                } else {
                    let next_timeout_increased = 2 * last_timeout.clone();
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
                let next_hello_timer_id = context.runtime().register_timer(next_timeout);

                *last_timeout = next_timeout;
                *hello_timer_id = next_hello_timer_id;
                *last_ssn = current_ssn;

                self.send_hello(context)?;
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
                                log::trace!(target: "vicinity_discovery", "Not responding to Hello from unchanged physical neighbor {} as we received an unexpected state sequence number", source);
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
                            log::trace!(target: "vicinity_discovery", "Not responding to Hello from unchanged physical neighbor {}", source);
                            return Ok(());
                        }
                    }
                }

                self.send_pn_disc_req(context, source)?;
            }
            (UseCaseEvent::Message(ProtocolMessage::PNDiscReq(req), _), _) => {
                if req.destination() != context.root_id() {
                    return Ok(());
                }
                self.send_pn_disc_rsp(context, req)?;
            }
            (UseCaseEvent::Hardware(HardwareEvent::InterfacesUp(_)), _) => {
                // send hello message immediately if interface comes up
                // todo add small random delay
                // todo reset hello timer if this fires too far away in the future
                self.send_hello(context)?;
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
                        Self::restricted_send_pn_disc_req(&context, nid.clone())?;
                    } else {
                        if let Some(contact) = context.routing_table().contact(&nid) {
                            Self::restricted_send_query_route_req(&context, contact.clone())?;
                        }
                    }

                    *current_tries += 1;
                    if *current_tries >= self.config.max_resync_tries {
                        max_retries_reached.push(nid.clone());
                    }
                }

                // FIXME this needs to be synced somehow to precompute_paths_and_path_ids
                for nid_max_retries_reached in max_retries_reached {
                    resync_queue.remove(&nid_max_retries_reached);
                }

                self.set_next_resync_timeout(&context);
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
                let source_id = payload.source().clone();

                // TODO figure out if this actually works with a cloned source_id
                if let std::collections::hash_map::Entry::Occupied(entry) =
                    resync_queue.entry(source_id.clone())
                {
                    let (expected_ssn, _) = entry.get();
                    if source_ssn >= &expected_ssn {
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::num::NonZeroUsize;
    use std::time::Duration;

    use crate::broadcaster::MPSCBroadcaster;
    use crate::context::{ContextConfig, SyncContext, UseCaseContext};
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::domain::{
        Contact, InMemoryPNTable, InsertionStrategyResult, NetworkInterface, NodeId, PNTable, Path,
        RoutingTable, StateSeqNr, TestInsertionStrategy,
    };
    use crate::forwarding::in_memory_tables::InMemoryFwdTables;
    use crate::hardware_events::HardwareEvent::InterfacesUp;
    use crate::messaging::source_route::SourceRoute;
    use crate::messaging::{
        AsyncProtocolMessageReceiver, HelloMessage, InMemoryMessageChannel, Nonce, ProtocolMessage,
        QueryRouteReqData, QueryRouteType, RTableData, ReqRspMessage,
    };
    use crate::runtime::ImmediateRuntime;
    use crate::use_cases::vicinity_discovery::{
        VDState, VicinityDiscovery, VicinityDiscoveryConfig, VICINITY_RADIUS,
    };
    use crate::use_cases::{ContactEvent, EventHandler, UseCase, UseCaseEvent};

    #[test]
    fn startup_test() {
        let root_id = NodeId::one();

        let routing_table = SingleBucketRT::<20>::new(root_id.clone());

        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let (broadcaster, broadcast_receiver) = MPSCBroadcaster::new(10);

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table: InMemoryPNTable::new(),
            insertion_strategy,
            message_sender: hub_sender,
            runtime: ImmediateRuntime::new(broadcaster.clone()),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case = VicinityDiscovery::new(VicinityDiscoveryConfig {
            max_timeout: Duration::from_secs(0),
            initial_timeout: Duration::from_secs(0),
            max_scatter: Duration::from_secs(0),
            ..Default::default()
        });

        assert!(use_case.start(&context).is_ok());

        let timer_id = match &use_case.state {
            VDState::Running(_, timer_id, _) => *timer_id,
            _ => panic!("Invalid state returned: {:?}", &use_case.state),
        };

        let event = broadcast_receiver
            .try_recv()
            .expect("failed to receive event");
        assert_eq!(event, UseCaseEvent::Timer(timer_id));
    }

    #[test]
    fn initial_hello_timeout_used() {
        let root_id = NodeId::one();

        let routing_table = SingleBucketRT::<20>::new(root_id.clone());

        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(10);

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table: InMemoryPNTable::new(),
            insertion_strategy,
            message_sender: hub_sender,
            runtime: ImmediateRuntime::new(broadcaster.clone()),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case = VicinityDiscovery::new(VicinityDiscoveryConfig {
            max_timeout: Duration::from_secs(0),
            initial_timeout: Duration::from_secs(33),
            max_scatter: Duration::from_secs(0),
            ..Default::default()
        });

        assert!(use_case.start(&context).is_ok());

        let (timer_duration, _timer_id) = match &use_case.state {
            VDState::Running(timer_duration, timer_id, _) => (*timer_duration, *timer_id),
            _ => panic!("Invalid state returned: {:?}", &use_case.state),
        };

        assert_eq!(
            timer_duration,
            Duration::from_secs(33),
            "Timeout should be configured initial timeout 33s but was {:?}",
            timer_duration
        );
    }

    #[test]
    fn timeout_for_hello_exponentially_increased_until_max_timeout() {
        let root_id = NodeId::one();

        let routing_table = SingleBucketRT::<20>::new(root_id.clone());

        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(10);

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table: InMemoryPNTable::new(),
            insertion_strategy,
            message_sender: hub_sender,
            runtime: ImmediateRuntime::new(broadcaster.clone()),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case = VicinityDiscovery::new(VicinityDiscoveryConfig {
            max_timeout: Duration::from_millis(30),
            initial_timeout: Duration::from_millis(8),
            max_scatter: Duration::from_millis(0),
            ..Default::default()
        });

        assert!(use_case.start(&context).is_ok());

        let (timer_duration, timer_id) = match &use_case.state {
            VDState::Running(timer_duration, timer_id, _) => (*timer_duration, *timer_id),
            _ => panic!("Invalid state returned: {:?}", &use_case.state),
        };
        assert_eq!(
            timer_duration,
            Duration::from_millis(8),
            "Timeout should be configured initial timeout 8ms but was {:?}",
            timer_duration
        );

        let handle_result = use_case.handle_event(&context, UseCaseEvent::Timer(timer_id));
        assert!(
            handle_result.is_ok(),
            "Handling timer event returned error: {:?}",
            handle_result
        );
        let (next_timer_duration, timer_id) = match &use_case.state {
            VDState::Running(timer_duration, timer_id, _) => (*timer_duration, *timer_id),
            _ => panic!("Invalid state returned: {:?}", &use_case.state),
        };
        assert_eq!(
            next_timer_duration,
            Duration::from_millis(16),
            "Timeout should be exponentially increased to 16ms but was: {:?}",
            next_timer_duration
        );

        let handle_result = use_case.handle_event(&context, UseCaseEvent::Timer(timer_id));
        assert!(
            handle_result.is_ok(),
            "Handling timer event returned error: {:?}",
            handle_result
        );
        let (next_timer_duration, _timer_id) = match &use_case.state {
            VDState::Running(timer_duration, timer_id, _) => (*timer_duration, *timer_id),
            _ => panic!("Invalid state returned: {:?}", &use_case.state),
        };
        assert_eq!(
            next_timer_duration,
            Duration::from_millis(30),
            "Timeout should be exponentially increased until max_timeout (30ms) but was: {:?}",
            next_timer_duration
        );
    }

    #[tokio::test]
    async fn responds_with_pn_disc_req() {
        crate::tests::init();

        // Answer should be a PNDiscReq if
        let root_id = NodeId::with_lsb(2);
        let sender_id = NodeId::with_lsb(4);

        let routing_table = SingleBucketRT::<20>::new(root_id.clone());

        let pn_table = InMemoryPNTable::new();

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let (hub_sender, mut hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table,
            insertion_strategy,
            message_sender: hub_sender,
            runtime,
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case = VicinityDiscovery::new(VicinityDiscoveryConfig {
            heuristic_calculation_bits: NonZeroUsize::new(34).unwrap(),
            ..Default::default()
        });

        assert!(use_case.start(&context).is_ok());
        assert_eq!(
            use_case.handle_event(
                &context,
                UseCaseEvent::Message(
                    ProtocolMessage::Hello(HelloMessage {
                        source: sender_id.clone(),
                        source_state_seq_nr: StateSeqNr::from(0),
                    }),
                    InMemoryMessageChannel::dummy_interface(),
                ),
            ),
            Ok(())
        );

        let pn_disc_req = hub_receiver
            .try_recv()
            .await
            .into_iter()
            .flatten()
            .find_map(|message| {
                if let ProtocolMessage::PNDiscReq(message) = &message.0 {
                    Some(message.clone())
                } else {
                    None
                }
            });
        assert!(pn_disc_req.is_some());
        let message = pn_disc_req.unwrap();
        assert_eq!(message.source(), &root_id);
        assert_eq!(message.destination(), &sender_id);
    }

    #[tokio::test]
    async fn doesnt_respond_with_pn_disc_req() {
        crate::tests::init();

        let root_id = NodeId::with_lsb(4);
        let sender_id = NodeId::with_lsb(2);

        let routing_table = SingleBucketRT::<20>::new(root_id.clone());

        let pn_table = InMemoryPNTable::new();

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let (hub_sender, mut hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table,
            insertion_strategy,
            message_sender: hub_sender,
            runtime,
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case = VicinityDiscovery::default();

        assert!(use_case.start(&context).is_ok());
        assert_eq!(
            use_case.handle_event(
                &context,
                UseCaseEvent::Message(
                    ProtocolMessage::Hello(HelloMessage {
                        source: sender_id.clone(),
                        source_state_seq_nr: StateSeqNr::from(0),
                    }),
                    InMemoryMessageChannel::dummy_interface(),
                ),
            ),
            Ok(())
        );

        let received = hub_receiver.try_recv().await;
        assert!(
            received.is_err() || received.as_ref().unwrap().is_none(),
            "unexpected message sent: {:?}",
            received
        );
    }

    #[test]
    fn heuristic_is_not_commutative() {
        // Important: Bot lsb are 00000..
        let own_id = NodeId::with_msb(1);
        let other_id = NodeId::with_msb(2);

        let bits = NonZeroUsize::new(32).unwrap();

        assert_ne!(
            super::deterministic_heuristic(&own_id, &other_id, bits),
            super::deterministic_heuristic(&other_id, &own_id, bits)
        );
    }

    #[tokio::test]
    async fn returns_pn_contacts() {
        crate::tests::init();

        let root_id = NodeId::with_msb(1);
        let source_id = NodeId::with_msb(2);

        let neighbors_port = InMemoryMessageChannel::dummy_interface();
        let neighbor_contacts = vec![
            Contact::new(Path::from([NodeId::with_msb(14)]), StateSeqNr::from(14)),
            Contact::new(Path::from([NodeId::with_msb(16)]), StateSeqNr::from(16)),
            Contact::new(Path::from([NodeId::with_msb(5)]), StateSeqNr::from(5)),
            Contact::new(Path::from([NodeId::with_msb(18)]), StateSeqNr::from(18)),
        ];

        let mut single_bucket_rt = SingleBucketRT::<20>::new(root_id.clone());
        let mut pn_table = InMemoryPNTable::new();

        for contact in &neighbor_contacts {
            pn_table.insert(contact.id().clone(), neighbors_port.clone());
            let insertion_result = single_bucket_rt.insert(contact.clone());
            assert!(
                insertion_result.is_ok(),
                "Insertion returned error: {:?}",
                insertion_result
            );
        }

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let (hub_sender, mut hub_receiver) = InMemoryMessageChannel::default().into_parts();
        let (broadcaster, _) = MPSCBroadcaster::new(10);
        let runtime = ImmediateRuntime::new(broadcaster.clone());
        let context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table: single_bucket_rt,
            pn_table,
            insertion_strategy,
            message_sender: hub_sender,
            runtime,
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case = VicinityDiscovery::default();
        let start_result = use_case.start(&context);
        assert!(
            start_result.is_ok(),
            "Start returned error: {:?}",
            start_result
        );

        let protocol_message = ProtocolMessage::PNDiscReq(ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: StateSeqNr::from(3),
            data: RTableData { contacts: vec![] },
            not_via: Default::default(),
            source_route: SourceRoute::from(Path::from([
                source_id.clone(),
                NodeId::with_msb(28),
                NodeId::with_msb(14),
                root_id.clone(),
            ]))
            .advanced()
            .advanced(),
        });
        let event = UseCaseEvent::Message(protocol_message.clone(), neighbors_port);

        let handle_result = use_case.handle_event(&context, event.clone());
        assert!(
            handle_result.is_ok(),
            "Handling returned an error: {:?}",
            handle_result
        );

        let sent_message = hub_receiver.try_recv().await;
        assert!(
            sent_message.is_ok(),
            "Trying to receive returned error: {:?}",
            sent_message
        );
        let sent_message = sent_message.unwrap();
        assert!(sent_message.is_some(), "No message sent by use case");
        let (sent_message, _) = sent_message.unwrap();

        assert_eq!(sent_message.nonce(), protocol_message.nonce());
        assert_eq!(
            sent_message.source_route(),
            Some(&SourceRoute::from(Path::from([
                root_id.clone(),
                NodeId::with_msb(14),
                NodeId::with_msb(28),
                source_id.clone(),
            ])))
        );
        let req = if let ProtocolMessage::PNDiscRsp(inner_req) = sent_message.clone() {
            Some(inner_req)
        } else {
            None
        };
        assert!(
            req.is_some(),
            "Returned a different ProtocolMessage than PNDiscRsp: {:?}",
            sent_message
        );
        let req = req.unwrap();
        // Check unordered equality
        assert_eq!(req.data.contacts.len(), neighbor_contacts.len());
        for neighbor in &neighbor_contacts {
            assert!(req.data.contacts.contains(&neighbor));
        }
    }

    #[tokio::test]
    async fn no_request_for_pns() {
        crate::tests::init();

        let root_id = NodeId::random();

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, mut hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let sync_context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table: SingleBucketRT::<1>::new(root_id),
            pn_table: InMemoryPNTable::new(),
            insertion_strategy: TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            message_sender: hub_sender,
            runtime,
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case = VicinityDiscovery::new(Default::default());

        assert_eq!(use_case.start(&sync_context), Ok(()));

        let event = UseCaseEvent::Contact(ContactEvent::New(Contact::new(
            Path::from(NodeId::random()),
            StateSeqNr::from(0),
        )));
        assert_eq!(use_case.handle_event(&sync_context, event), Ok(()));

        let received = hub_receiver.try_recv().await;
        assert!(
            received.is_err() || received.as_ref().unwrap().is_none(),
            "Unexpected message sent: {:?}",
            received
        );
    }

    #[tokio::test]
    async fn no_request_for_outside_vicinity() {
        crate::tests::init();

        let root_id = NodeId::random();

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, mut hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let sync_context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table: SingleBucketRT::<1>::new(root_id),
            pn_table: InMemoryPNTable::new(),
            insertion_strategy: TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            message_sender: hub_sender,
            runtime,
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case = VicinityDiscovery::new(Default::default());

        assert_eq!(use_case.start(&sync_context), Ok(()));

        // Build path bigger than vicinity radius
        let mut path = Path::from(NodeId::random());
        for _ in 0..VICINITY_RADIUS {
            path.push(NodeId::random());
        }

        let event =
            UseCaseEvent::Contact(ContactEvent::New(Contact::new(path, StateSeqNr::from(0))));
        assert_eq!(use_case.handle_event(&sync_context, event), Ok(()));

        let received = hub_receiver.try_recv().await;
        assert!(
            received.is_err() || received.as_ref().unwrap().is_none(),
            "Unexpected message sent: {:?}",
            received
        );
    }

    #[tokio::test]
    async fn request_for_inside_vicinity() {
        crate::tests::init();

        let root_id = NodeId::random();

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, mut hub_receiver) = InMemoryMessageChannel::default().into_parts();

        // At least one has contact has to be present and valid
        // Otherwise the use case thinks the node is isolated
        let neighbor_id = NodeId::random();
        let mut routing_table = SingleBucketRT::<1>::new(root_id.clone());
        assert!(routing_table
            .insert(Contact::new(
                Path::from(neighbor_id.clone()),
                StateSeqNr::from(0),
            ))
            .is_ok());
        let mut pn_table = InMemoryPNTable::new();
        pn_table.insert(neighbor_id.clone(), NetworkInterface::with_name("test"));

        let sync_context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table,
            insertion_strategy: TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            message_sender: hub_sender,
            runtime,
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case = VicinityDiscovery::new(Default::default());

        assert_eq!(use_case.start(&sync_context), Ok(()));

        // Build path bigger than vicinity radius
        let contact_id = NodeId::random();
        let path = Path::from([neighbor_id.clone(), contact_id.clone()]);

        let event =
            UseCaseEvent::Contact(ContactEvent::New(Contact::new(path, StateSeqNr::from(0))));
        assert_eq!(use_case.handle_event(&sync_context, event), Ok(()));

        let (received, _) = hub_receiver
            .try_recv()
            .await
            .expect("receiving should work")
            .expect("should return an actual message");

        if let ProtocolMessage::QueryRouteReq(ReqRspMessage {
            data:
                QueryRouteReqData {
                    query_type: QueryRouteType::PhysicalNeighbors,
                },
            ..
        }) = &received
        {
            assert_eq!(received.source(), &root_id);
            assert_eq!(received.destination(), Some(&contact_id));
            let route = received.source_route();
            assert!(route.is_some(), "received source route is empty");
            let route = route.unwrap();
            assert_eq!(route.current_hop(), &neighbor_id);
        } else {
            panic!("Invalid response received: {:?}", received);
        }
    }

    #[tokio::test]
    async fn answers_query_route_req_with_pns() {
        crate::tests::init();

        let root_id = NodeId::with_msb(1);
        let source_id = NodeId::with_msb(2);

        let neighbors_port = InMemoryMessageChannel::dummy_interface();
        let neighbor_contacts = vec![
            Contact::new(Path::from([NodeId::with_msb(14)]), StateSeqNr::from(14)),
            Contact::new(Path::from([NodeId::with_msb(16)]), StateSeqNr::from(16)),
            Contact::new(Path::from([NodeId::with_msb(5)]), StateSeqNr::from(5)),
            Contact::new(Path::from([NodeId::with_msb(18)]), StateSeqNr::from(18)),
        ];

        let mut single_bucket_rt = SingleBucketRT::<20>::new(root_id.clone());
        let mut pn_table = InMemoryPNTable::new();

        for contact in &neighbor_contacts {
            pn_table.insert(contact.id().clone(), neighbors_port.clone());
            let insertion_result = single_bucket_rt.insert(contact.clone());
            assert!(
                insertion_result.is_ok(),
                "Insertion returned error: {:?}",
                insertion_result
            );
        }

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let (hub_sender, mut hub_receiver) = InMemoryMessageChannel::default().into_parts();
        let (broadcaster, _) = MPSCBroadcaster::new(10);
        let runtime = ImmediateRuntime::new(broadcaster.clone());
        let context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table: single_bucket_rt,
            pn_table,
            insertion_strategy,
            message_sender: hub_sender,
            runtime,
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case = VicinityDiscovery::default();
        let start_result = use_case.start(&context);
        assert!(
            start_result.is_ok(),
            "Start returned error: {:?}",
            start_result
        );

        let route = SourceRoute::from(Path::from([
            source_id.clone(),
            NodeId::with_msb(28),
            NodeId::with_msb(14),
            root_id.clone(),
        ]))
        .advanced()
        .advanced();
        let protocol_message = ProtocolMessage::QueryRouteReq(ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: StateSeqNr::from(3),
            data: QueryRouteReqData {
                query_type: QueryRouteType::PhysicalNeighbors,
            },
            not_via: Default::default(),
            source_route: route,
        });
        let event = UseCaseEvent::Message(protocol_message.clone(), neighbors_port);

        let handle_result = use_case.handle_event(&context, event.clone());
        assert!(
            handle_result.is_ok(),
            "Handling returned an error: {:?}",
            handle_result
        );

        let sent_message = hub_receiver.try_recv().await;
        assert!(
            sent_message.is_ok(),
            "Trying to receive returned error: {:?}",
            sent_message
        );
        let sent_message = sent_message.unwrap();
        assert!(sent_message.is_some(), "No message sent by use case");
        let (sent_message, _) = sent_message.unwrap();

        assert_eq!(sent_message.nonce(), protocol_message.nonce());
        assert_eq!(
            sent_message.source_route(),
            Some(&SourceRoute::from(Path::from([
                root_id.clone(),
                NodeId::with_msb(14),
                NodeId::with_msb(28),
                source_id.clone(),
            ])))
        );
        let req = if let ProtocolMessage::QueryRouteRsp(inner_req) = sent_message.clone() {
            Some(inner_req)
        } else {
            None
        };
        assert!(
            req.is_some(),
            "Returned a different ProtocolMessage than QueryRouteRsp: {:?}",
            sent_message
        );
        let req = req.unwrap();
        // Check unordered equality
        assert_eq!(req.data.contacts.len(), neighbor_contacts.len());
        for neighbor in &neighbor_contacts {
            assert!(req.data.contacts.contains(&neighbor));
        }
    }

    #[tokio::test]
    async fn ignores_query_route_req_for_others() {
        crate::tests::init();

        let root_id = NodeId::with_msb(1);
        let source_id = NodeId::with_msb(2);

        let single_bucket_rt = SingleBucketRT::<20>::new(root_id.clone());
        let pn_table = InMemoryPNTable::new();
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let (hub_sender, mut hub_receiver) = InMemoryMessageChannel::default().into_parts();
        let (broadcaster, _) = MPSCBroadcaster::new(10);
        let runtime = ImmediateRuntime::new(broadcaster.clone());
        let context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table: single_bucket_rt,
            pn_table,
            insertion_strategy,
            message_sender: hub_sender,
            runtime,
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case = VicinityDiscovery::default();
        let start_result = use_case.start(&context);
        assert!(
            start_result.is_ok(),
            "Start returned error: {:?}",
            start_result
        );

        let protocol_message = ProtocolMessage::QueryRouteReq(ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: StateSeqNr::from(3),
            data: QueryRouteReqData {
                query_type: QueryRouteType::PhysicalNeighbors,
            },
            not_via: Default::default(),
            source_route: SourceRoute::from(Path::from([
                source_id.clone(),
                NodeId::with_msb(28),
                NodeId::with_msb(14),
            ])),
        });
        let event = UseCaseEvent::Message(
            protocol_message.clone(),
            InMemoryMessageChannel::dummy_interface(),
        );

        let handle_result = use_case.handle_event(&context, event.clone());
        assert!(
            handle_result.is_ok(),
            "Handling returned an error: {:?}",
            handle_result
        );

        let sent_message = hub_receiver.try_recv().await;
        assert!(
            sent_message.is_ok(),
            "Trying to receive returned error: {:?}",
            sent_message
        );
        let sent_message = sent_message.unwrap();
        assert!(
            sent_message.is_none(),
            "No message should be sent bei use case"
        );
    }

    #[tokio::test]
    async fn send_hello_on_interface_up() {
        crate::tests::init();

        let root_id = NodeId::with_msb(1);
        let source_id = NodeId::with_msb(2);

        let single_bucket_rt = SingleBucketRT::<20>::new(root_id.clone());
        let pn_table = InMemoryPNTable::new();
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let (hub_sender, mut hub_receiver) = InMemoryMessageChannel::default().into_parts();
        let (broadcaster, _) = MPSCBroadcaster::new(10);
        let runtime = ImmediateRuntime::new(broadcaster.clone());
        let context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table: single_bucket_rt,
            pn_table,
            insertion_strategy,
            message_sender: hub_sender,
            runtime,
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case = VicinityDiscovery::default();
        let start_result = use_case.start(&context);
        assert!(
            start_result.is_ok(),
            "Start returned error: {:?}",
            start_result
        );

        let mut interfaces = HashSet::with_capacity(1);
        interfaces.insert(InMemoryMessageChannel::dummy_interface());
        let event = UseCaseEvent::Hardware(InterfacesUp(interfaces));

        let handle_result = use_case.handle_event(&context, event.clone());
        assert!(
            handle_result.is_ok(),
            "Handling returned an error: {:?}",
            handle_result
        );

        let sent_message = hub_receiver.try_recv().await;
        assert!(
            sent_message.is_ok(),
            "Trying to receive returned error: {:?}",
            sent_message
        );
        let sent_message = sent_message.unwrap();
        assert!(
            sent_message.is_some(),
            "We expect a hello message, something should be sent."
        );

        let (sent_message, _) = sent_message.unwrap();
        assert!(
            matches!(sent_message, ProtocolMessage::Hello(_)),
            "Expecting a hello message but received: {:?}",
            sent_message
        )
    }

    #[test]
    fn reset_timeout_on_ssn_change() {
        let root_id = NodeId::one();

        let routing_table = SingleBucketRT::<20>::new(root_id.clone());

        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(10);

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table: InMemoryPNTable::new(),
            insertion_strategy,
            message_sender: hub_sender,
            runtime: ImmediateRuntime::new(broadcaster.clone()),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case = VicinityDiscovery::new(VicinityDiscoveryConfig {
            max_timeout: Duration::from_millis(30),
            initial_timeout: Duration::from_millis(8),
            max_scatter: Duration::from_millis(0),
            ..Default::default()
        });

        assert!(use_case.start(&context).is_ok());

        let (timer_duration, timer_id) = match &use_case.state {
            VDState::Running(timer_duration, timer_id, _) => (*timer_duration, *timer_id),
            _ => panic!("Invalid state returned: {:?}", &use_case.state),
        };
        assert_eq!(
            timer_duration,
            Duration::from_millis(8),
            "Timeout should be configured initial timeout 8ms but was {:?}",
            timer_duration
        );

        let handle_result = use_case.handle_event(&context, UseCaseEvent::Timer(timer_id));
        assert!(
            handle_result.is_ok(),
            "Handling timer event returned error: {:?}",
            handle_result
        );
        let (next_timer_duration, timer_id) = match &use_case.state {
            VDState::Running(timer_duration, timer_id, _) => (*timer_duration, *timer_id),
            _ => panic!("Invalid state returned: {:?}", &use_case.state),
        };
        assert_eq!(
            next_timer_duration,
            Duration::from_millis(16),
            "Timeout should be exponentially increased to 16ms but was: {:?}",
            next_timer_duration
        );

        // now we increase the ssn
        *context.pn_table_mut().state_seq_nr_mut() += 1;

        let handle_result = use_case.handle_event(&context, UseCaseEvent::Timer(timer_id));
        assert!(
            handle_result.is_ok(),
            "Handling timer event returned error: {:?}",
            handle_result
        );
        let (next_timer_duration, timer_id) = match &use_case.state {
            VDState::Running(timer_duration, timer_id, _) => (*timer_duration, *timer_id),
            _ => panic!("Invalid state returned: {:?}", &use_case.state),
        };
        assert_eq!(
            next_timer_duration, use_case.config.initial_timeout,
            "Timeout should be reset to initial timeout since the ssn changed but was: {:?}",
            next_timer_duration
        );

        // test of we exponentially increase again
        let handle_result = use_case.handle_event(&context, UseCaseEvent::Timer(timer_id));
        assert!(
            handle_result.is_ok(),
            "Handling timer event returned error: {:?}",
            handle_result
        );
        let (next_timer_duration, _timer_id) = match &use_case.state {
            VDState::Running(timer_duration, timer_id, _) => (*timer_duration, *timer_id),
            _ => panic!("Invalid state returned: {:?}", &use_case.state),
        };
        assert_eq!(
            next_timer_duration,
            Duration::from_millis(16),
            "Timeout should be exponentially increased to 16ms again but was: {:?}",
            next_timer_duration
        );
    }

    #[tokio::test]
    async fn dont_send_pn_disc_req_on_unchanged_neighbor() {
        crate::tests::init();

        // Answer should be a PNDiscReq if
        let root_id = NodeId::with_lsb(2);
        let sender_id = NodeId::with_lsb(4);

        let routing_table = SingleBucketRT::<20>::new(root_id.clone());

        let pn_table = InMemoryPNTable::new();

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let (hub_sender, mut hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table,
            insertion_strategy,
            message_sender: hub_sender,
            runtime,
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case = VicinityDiscovery::new(VicinityDiscoveryConfig {
            heuristic_calculation_bits: NonZeroUsize::new(34).unwrap(),
            ..Default::default()
        });

        assert!(use_case.start(&context).is_ok());

        let hello_msg = ProtocolMessage::Hello(HelloMessage {
            source: sender_id.clone(),
            source_state_seq_nr: StateSeqNr::from(0),
        });
        let event = UseCaseEvent::Message(hello_msg, InMemoryMessageChannel::dummy_interface());
        let handle_result = use_case.handle_event(&context, event);
        assert!(
            handle_result.is_ok(),
            "Handling returned an error: {:?}",
            handle_result
        );

        // initially we expect a PNDiscReq to be sent
        let sent_message = hub_receiver.try_recv().await;
        assert!(
            sent_message.is_ok(),
            "Trying to receive returned error: {:?}",
            sent_message
        );
        let sent_message = sent_message.unwrap();
        assert!(
            sent_message.is_some(),
            "We expect a PNDiscReq message, something should be sent."
        );

        let (sent_message, _) = sent_message.unwrap();
        assert!(
            matches!(sent_message, ProtocolMessage::PNDiscReq(_)),
            "Expecting a PNDiscReq message but received: {:?}",
            sent_message
        );

        // fake adding of PN
        context
            .pn_table_mut()
            .insert(sender_id.clone(), InMemoryMessageChannel::dummy_interface());
        let contact = Contact::new(
            Path::from([root_id.clone(), sender_id.clone()]),
            sent_message.source_state_seq_nr().clone(),
        );
        let insertion_result = context.routing_table_mut().insert(contact);
        assert!(
            insertion_result.is_ok(),
            "Expecting no issue insertion contact: {:?}",
            insertion_result
        );

        // now we shouldn't send out another message,
        // since we already added the contact and nothing changed to the ssn

        let hello_msg = ProtocolMessage::Hello(HelloMessage {
            source: sender_id.clone(),
            source_state_seq_nr: StateSeqNr::from(0),
        });
        let event = UseCaseEvent::Message(hello_msg, InMemoryMessageChannel::dummy_interface());
        let handle_result = use_case.handle_event(&context, event);
        assert!(
            handle_result.is_ok(),
            "Handling returned an error: {:?}",
            handle_result
        );

        let sent_message = hub_receiver.try_recv().await.ok().flatten();
        assert!(
            sent_message.is_none(),
            "Trying to receive returned a message: {:?}",
            sent_message
        );
    }
}
