use std::error::Error;
use std::fmt::{Display, Formatter};
use std::marker::PhantomData;
use std::num::NonZeroUsize;
use std::time::Duration;

use rand::Rng;

use crate::context::UseCaseContext;
use crate::domain::{
    node_id, Contact, ContactState, NodeId, Path, RoutingTable, DEFAULT_BUCKET_SIZE,
};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{
    HelloMessage, Nonce, ProtocolMessage, ProtocolMessageSender, QueryRouteReqData, QueryRouteType,
    RTableData, ReqRspMessage,
};
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{ContactEvent, TimerId, UseCase, UseCaseEvent, UseCaseState};

/// Radius of the neighborhood considered as vicinity.
///
/// A radius of **3** means:
///
/// - Contacts with a distance **<= 3** hops (*path length <= 4*) are in the vicinity.
/// - Contacts with a distance **< 3** hops (*path length < 4*) receive QueryRouteReqs.
///     Except the physical neighbors with a distance of 0 hops (*path length == 1*), which
///     are handled by the [HandleHelloUseCase] (*Hello* and *PNDiscReq/-Rsp*).
pub const VICINITY_RADIUS: usize = 3;

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
}

impl Default for VicinityDiscoveryConfig {
    fn default() -> Self {
        Self {
            max_timeout: Duration::from_secs(30),
            initial_timeout: Duration::from_millis(250),
            max_scatter: Duration::from_millis(225),
            heuristic_calculation_bits: NonZeroUsize::new(32).unwrap(),
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
    Running(Duration, TimerId),
    Error,
}

impl UseCaseState for VDState {
    fn is_error(&self) -> bool {
        self == &Self::Error
    }
}

/// The vicinity discovery (VD) use case.
///
/// Handles the discovery of the physical neighborhood (*vicinity*) beyond the direct
/// physical neighbors.
///
/// If a new [Contact] was added to the [RoutingTable] or an existing one was updated and
/// has a physical distance in the range of [1, [VICINITY_RADIUS]] hops (*path length is in
/// [2, [VICINITY_RADIUS] + 1]) a QueryRouteReq is sent to them to get their physical neighbors.
///
/// Physical Neighbors are already handled by the [HandleHelloUseCase] which is why
/// the range starts at 1 hop.
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
    /// Create a new vicinity discovery use case in [VDState::Idle].
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

    /// Calculates the duration to wait before sending the next `PNHello`.
    ///
    /// Exponentially increases the last duration until the configured [PeriodicPNAdvertisingConfig::probing_timeout]
    /// is reached and then that duration will be used.
    ///
    /// Also introduces a randomized scatter to the duration given through
    /// [PeriodicPNAdvertisingConfig::max_scatter].
    fn next_timeout_duration(&self, last_duration: Duration) -> Duration {
        let next_timeout_increased = 2 * last_duration;
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
    }
}

impl<C, const BUCKET_SIZE: usize> VicinityDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::MessageSender: ProtocolMessageSender,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
{
    fn send_query_route_req(&mut self, context: &C, contact: Contact) -> Result<(), VDError> {
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
            self.state = VDState::Error;
            return Err(VDError::NeighborInconsistency);
        }

        // Request only physical Neighborhood of that Node
        let request = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
            data: QueryRouteReqData {
                query_type: QueryRouteType::PhysicalNeighbors,
            },
            source_route: route,
        };

        log::trace!(target: "vicinity_discovery", "Sending message {:?}", request);

        if let Err(e) = context.message_sender_mut().send(request) {
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
            source_route: SourceRoute::from_reversed(request.source_route),
        });

        log::trace!(target: "vicinity_discovery", "Sending: {:?}", message);

        if let Err(e) = context.message_sender_mut().send(message) {
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
        log::trace!(target: "vicinity_discovery", "Sending message {:?}", message);
        if let Err(e) = context.message_sender_mut().send(message) {
            log::error!(target: "vicinity_discovery",
                "MessageSender failed: {}",
                e
            );
            return Err(VDError::MessageSendFailed);
        }

        Ok(())
    }

    fn send_pn_disc_req(&self, context: &C, source: NodeId) -> Result<(), VDError> {
        // Answer with a PNDiscReq to ensure bidirectional connectivity
        let pn_contacts = context
            .pn_table()
            .into_iter()
            .filter_map(|(id, _)| context.routing_table().contact(id).cloned())
            .collect::<Vec<_>>();

        let message = ProtocolMessage::PNDiscReq(ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
            data: RTableData {
                contacts: pn_contacts,
            },
            // Source route is ignored, as only physical neighbors get these
            source_route: SourceRoute::from(Path::from([context.root_id().clone(), source])),
        });

        log::trace!(
            target: "vicinity_discovery",
            "Sending message: {:?}",
            message
        );

        if let Err(e) = context.message_sender_mut().send(message) {
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
            source_route: SourceRoute::from_reversed(req.source_route),
        });

        log::trace!(target: "vicinity_discovery", "Sending: {:?}", response);

        if let Err(e) = context.message_sender_mut().send(response) {
            log::error!(target: "vicinity_discovery", "Failed to send PNDiscRsp: {}", e);
            return Err(VDError::MessageSendFailed);
        }

        Ok(())
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase for VicinityDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::MessageSender: ProtocolMessageSender,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
{
    type Context = C;
    type Error = VDError;
    type State = VDState;
    type Value = ();

    fn start(&mut self, context: &Self::Context) -> Result<(), Self::Error> {
        let timer_id = context
            .runtime()
            .register_timer(self.config.initial_timeout);

        self.state = VDState::Running(self.config.initial_timeout, timer_id);

        Ok(())
    }

    fn handle_event(
        &mut self,
        context: &Self::Context,
        event: UseCaseEvent,
    ) -> Result<(), Self::Error> {
        match (event, &self.state) {
            (UseCaseEvent::Contact(ContactEvent::New(contact)), _) => {
                self.send_query_route_req(context, contact)?;
            }
            (UseCaseEvent::Contact(ContactEvent::Updated { new, old }), _) => {
                // Only if path changed. Path length checks and everything else is done in discover
                if new.path() != old.path() {
                    self.send_query_route_req(context, new)?;
                }
            }
            (UseCaseEvent::Message(ProtocolMessage::QueryRouteReq(request), _), _) => {
                if request.destination() != context.root_id() {
                    return Ok(());
                }

                self.send_query_route_rsp(context, request)?;
            }
            (UseCaseEvent::Timer(id), VDState::Running(last_timeout, timer_id)) => {
                if timer_id == &id {
                    self.send_hello(context)?;

                    let next_timeout = self.next_timeout_duration(*last_timeout);
                    let timer_id = context.runtime().register_timer(next_timeout);
                    self.state = VDState::Running(next_timeout, timer_id);
                }
            }
            (UseCaseEvent::Message(ProtocolMessage::Hello(HelloMessage { source, .. }), _), _) => {
                if !deterministic_heuristic(
                    context.root_id(),
                    &source,
                    self.config.heuristic_calculation_bits,
                ) {
                    log::trace!(target: "vicinity_discovery", "Not responding to Hello from {}", source);
                    return Ok(());
                }

                self.send_pn_disc_req(context, source)?;
            }
            (UseCaseEvent::Message(ProtocolMessage::PNDiscReq(req), _), _) => {
                if req.destination() != context.root_id() {
                    return Ok(());
                }
                self.send_pn_disc_rsp(context, req)?;
            }
            _ => {}
        }

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;
    use std::time::Duration;

    use crate::broadcaster::MPSCBroadcaster;
    use crate::context::SyncContext;
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::domain::{
        Contact, InsertionStrategyResult, NetworkInterface, NodeId, PNTable, Path, RoutingTable,
        StateSeqNr, TestInsertionStrategy,
    };
    use crate::messaging::source_route::SourceRoute;
    use crate::messaging::tests::ArcSyncInMemoryMessageHub;
    use crate::messaging::{
        HelloMessage, InMemoryMessageHub, Nonce, ProtocolMessage, ProtocolMessageReceiver,
        QueryRouteReqData, QueryRouteType, RTableData, ReqRspMessage,
    };
    use crate::runtime::ImmediateRuntime;
    use crate::use_cases::vicinity_discovery::{
        VDState, VicinityDiscovery, VicinityDiscoveryConfig, VICINITY_RADIUS,
    };
    use crate::use_cases::{ContactEvent, UseCase, UseCaseEvent};

    #[test]
    fn startup_test() {
        let root_id = NodeId::one();

        let routing_table = SingleBucketRT::<20>::new(root_id.clone());

        let hub = ArcSyncInMemoryMessageHub::new();

        let (broadcaster, broadcast_receiver) = MPSCBroadcaster::new(10);

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let context = SyncContext::new(
            root_id.clone(),
            routing_table,
            PNTable::new(),
            insertion_strategy,
            hub.clone(),
            ImmediateRuntime::new(broadcaster.clone()),
        );

        let mut use_case = VicinityDiscovery::new(VicinityDiscoveryConfig {
            max_timeout: Duration::from_secs(0),
            initial_timeout: Duration::from_secs(0),
            max_scatter: Duration::from_secs(0),
            ..Default::default()
        });

        assert!(use_case.start(&context).is_ok());

        let timer_id = match &use_case.state {
            VDState::Running(_, timer_id) => *timer_id,
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

        let hub = ArcSyncInMemoryMessageHub::new();

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(10);

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let context = SyncContext::new(
            root_id.clone(),
            routing_table,
            PNTable::new(),
            insertion_strategy,
            hub.clone(),
            ImmediateRuntime::new(broadcaster.clone()),
        );

        let mut use_case = VicinityDiscovery::new(VicinityDiscoveryConfig {
            max_timeout: Duration::from_secs(0),
            initial_timeout: Duration::from_secs(33),
            max_scatter: Duration::from_secs(0),
            ..Default::default()
        });

        assert!(use_case.start(&context).is_ok());

        let (timer_duration, _timer_id) = match &use_case.state {
            VDState::Running(timer_duration, timer_id) => (*timer_duration, *timer_id),
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

        let hub = ArcSyncInMemoryMessageHub::new();

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(10);

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let context = SyncContext::new(
            root_id.clone(),
            routing_table,
            PNTable::new(),
            insertion_strategy,
            hub.clone(),
            ImmediateRuntime::new(broadcaster.clone()),
        );

        let mut use_case = VicinityDiscovery::new(VicinityDiscoveryConfig {
            max_timeout: Duration::from_millis(30),
            initial_timeout: Duration::from_millis(8),
            max_scatter: Duration::from_millis(0),
            ..Default::default()
        });

        assert!(use_case.start(&context).is_ok());

        let (timer_duration, timer_id) = match &use_case.state {
            VDState::Running(timer_duration, timer_id) => (*timer_duration, *timer_id),
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
            VDState::Running(timer_duration, timer_id) => (*timer_duration, *timer_id),
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
            VDState::Running(timer_duration, timer_id) => (*timer_duration, *timer_id),
            _ => panic!("Invalid state returned: {:?}", &use_case.state),
        };
        assert_eq!(
            next_timer_duration,
            Duration::from_millis(30),
            "Timeout should be exponentially increased until max_timeout (30ms) but was: {:?}",
            next_timer_duration
        );
    }

    #[test]
    fn responds_with_pn_disc_req() {
        crate::tests::init();

        // Answer should be a PNDiscReq if
        let root_id = NodeId::with_lsb(2);
        let sender_id = NodeId::with_lsb(4);

        let routing_table = SingleBucketRT::<20>::new(root_id.clone());

        let pn_table = PNTable::new();

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let message_hub = ArcSyncInMemoryMessageHub::new();

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let context = SyncContext::new(
            root_id.clone(),
            routing_table,
            pn_table,
            insertion_strategy,
            message_hub.clone(),
            runtime,
        );

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
                    InMemoryMessageHub::dummy_interface(),
                ),
            ),
            Ok(())
        );

        let pn_disc_req = message_hub
            .messages()
            .iter()
            .find_map(|message| {
                if let ProtocolMessage::PNDiscReq(message) = &message.0 {
                    Some(message)
                } else {
                    None
                }
            })
            .cloned();
        assert!(pn_disc_req.is_some());
        let message = pn_disc_req.unwrap();
        assert_eq!(message.source(), &root_id);
        assert_eq!(message.destination(), &sender_id);
    }

    #[test]
    fn doesnt_respond_with_pn_disc_req() {
        crate::tests::init();

        let root_id = NodeId::with_lsb(4);
        let sender_id = NodeId::with_lsb(2);

        let routing_table = SingleBucketRT::<20>::new(root_id.clone());

        let pn_table = PNTable::new();

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let message_hub = ArcSyncInMemoryMessageHub::new();

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let context = SyncContext::new(
            root_id.clone(),
            routing_table,
            pn_table,
            insertion_strategy,
            message_hub.clone(),
            runtime,
        );

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
                    InMemoryMessageHub::dummy_interface(),
                ),
            ),
            Ok(())
        );

        assert!(message_hub.messages().is_empty());
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

    #[test]
    fn returns_pn_contacts() {
        crate::tests::init();

        let root_id = NodeId::with_msb(1);
        let source_id = NodeId::with_msb(2);

        let neighbors_port = InMemoryMessageHub::dummy_interface();
        let neighbor_contacts = vec![
            Contact::new(Path::from([NodeId::with_msb(14)]), StateSeqNr::from(14)),
            Contact::new(Path::from([NodeId::with_msb(16)]), StateSeqNr::from(16)),
            Contact::new(Path::from([NodeId::with_msb(5)]), StateSeqNr::from(5)),
            Contact::new(Path::from([NodeId::with_msb(18)]), StateSeqNr::from(18)),
        ];

        let mut single_bucket_rt = SingleBucketRT::<20>::new(root_id.clone());
        let mut pn_table = PNTable::new();

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
        let mut message_hub = ArcSyncInMemoryMessageHub::new();
        let (broadcaster, _) = MPSCBroadcaster::new(10);
        let runtime = ImmediateRuntime::new(broadcaster.clone());
        let context = SyncContext::new(
            root_id.clone(),
            single_bucket_rt,
            pn_table,
            insertion_strategy,
            message_hub.clone(),
            runtime,
        );

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
            source_route: SourceRoute::from(Path::from([
                source_id.clone(),
                NodeId::with_msb(28),
                NodeId::with_msb(14),
                root_id.clone(),
            ])),
        });
        let event = UseCaseEvent::Message(protocol_message.clone(), neighbors_port);

        let handle_result = use_case.handle_event(&context, event.clone());
        assert!(
            handle_result.is_ok(),
            "Handling returned an error: {:?}",
            handle_result
        );

        let sent_message = message_hub.try_recv();
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

    #[test]
    fn no_request_for_pns() {
        crate::tests::init();

        let root_id = NodeId::random();

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let hub = ArcSyncInMemoryMessageHub::new();

        let sync_context = SyncContext::new(
            root_id.clone(),
            SingleBucketRT::<1>::new(root_id),
            PNTable::new(),
            TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            hub.clone(),
            runtime,
        );

        let mut use_case = VicinityDiscovery::new(Default::default());

        assert_eq!(use_case.start(&sync_context), Ok(()));

        let event = UseCaseEvent::Contact(ContactEvent::New(Contact::new(
            Path::from(NodeId::random()),
            StateSeqNr::from(0),
        )));
        assert_eq!(use_case.handle_event(&sync_context, event), Ok(()));

        assert!(hub.messages().is_empty());
    }

    #[test]
    fn no_request_for_outside_vicinity() {
        crate::tests::init();

        let root_id = NodeId::random();

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let hub = ArcSyncInMemoryMessageHub::new();

        let sync_context = SyncContext::new(
            root_id.clone(),
            SingleBucketRT::<1>::new(root_id),
            PNTable::new(),
            TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            hub.clone(),
            runtime,
        );

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

        assert!(
            hub.messages().is_empty(),
            "Expected no message generation for request outside vicinity: {:?}",
            hub.messages()
        );
    }

    #[test]
    fn request_for_inside_vicinity() {
        crate::tests::init();

        let root_id = NodeId::random();

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let mut hub = ArcSyncInMemoryMessageHub::new();

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
        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), NetworkInterface::new("test"));

        let sync_context = SyncContext::new(
            root_id.clone(),
            routing_table,
            pn_table,
            TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            hub.clone(),
            runtime,
        );

        let mut use_case = VicinityDiscovery::new(Default::default());

        assert_eq!(use_case.start(&sync_context), Ok(()));

        // Build path bigger than vicinity radius
        let contact_id = NodeId::random();
        let path = Path::from([neighbor_id.clone(), contact_id.clone()]);

        let event =
            UseCaseEvent::Contact(ContactEvent::New(Contact::new(path, StateSeqNr::from(0))));
        assert_eq!(use_case.handle_event(&sync_context, event), Ok(()));

        let (received, _) = hub
            .recv_timeout(Some(Duration::from_secs(1)))
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

    #[test]
    fn answers_query_route_req_with_pns() {
        crate::tests::init();

        let root_id = NodeId::with_msb(1);
        let source_id = NodeId::with_msb(2);

        let neighbors_port = InMemoryMessageHub::dummy_interface();
        let neighbor_contacts = vec![
            Contact::new(Path::from([NodeId::with_msb(14)]), StateSeqNr::from(14)),
            Contact::new(Path::from([NodeId::with_msb(16)]), StateSeqNr::from(16)),
            Contact::new(Path::from([NodeId::with_msb(5)]), StateSeqNr::from(5)),
            Contact::new(Path::from([NodeId::with_msb(18)]), StateSeqNr::from(18)),
        ];

        let mut single_bucket_rt = SingleBucketRT::<20>::new(root_id.clone());
        let mut pn_table = PNTable::new();

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
        let mut message_hub = ArcSyncInMemoryMessageHub::new();
        let (broadcaster, _) = MPSCBroadcaster::new(10);
        let runtime = ImmediateRuntime::new(broadcaster.clone());
        let context = SyncContext::new(
            root_id.clone(),
            single_bucket_rt,
            pn_table,
            insertion_strategy,
            message_hub.clone(),
            runtime,
        );

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
            source_route: SourceRoute::from(Path::from([
                source_id.clone(),
                NodeId::with_msb(28),
                NodeId::with_msb(14),
                root_id.clone(),
            ])),
        });
        let event = UseCaseEvent::Message(protocol_message.clone(), neighbors_port);

        let handle_result = use_case.handle_event(&context, event.clone());
        assert!(
            handle_result.is_ok(),
            "Handling returned an error: {:?}",
            handle_result
        );

        let sent_message = message_hub.try_recv();
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

    #[test]
    fn ignores_query_route_req_for_others() {
        crate::tests::init();

        let root_id = NodeId::with_msb(1);
        let source_id = NodeId::with_msb(2);

        let single_bucket_rt = SingleBucketRT::<20>::new(root_id.clone());
        let pn_table = PNTable::new();
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let mut message_hub = ArcSyncInMemoryMessageHub::new();
        let (broadcaster, _) = MPSCBroadcaster::new(10);
        let runtime = ImmediateRuntime::new(broadcaster.clone());
        let context = SyncContext::new(
            root_id.clone(),
            single_bucket_rt,
            pn_table,
            insertion_strategy,
            message_hub.clone(),
            runtime,
        );

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
            source_route: SourceRoute::from(Path::from([
                source_id.clone(),
                NodeId::with_msb(28),
                NodeId::with_msb(14),
            ])),
        });
        let event = UseCaseEvent::Message(
            protocol_message.clone(),
            InMemoryMessageHub::dummy_interface(),
        );

        let handle_result = use_case.handle_event(&context, event.clone());
        assert!(
            handle_result.is_ok(),
            "Handling returned an error: {:?}",
            handle_result
        );

        let sent_message = message_hub.try_recv();
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
}
