use std::collections::HashSet;
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::marker::PhantomData;
use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::time::Duration;

use crate::context::UseCaseContext;
use crate::domain::RoutingTable;
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{
    FindNodeReqData, Nonce, ProtocolMessage, ProtocolMessageSender, ReqRspMessage,
};
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{TimerId, UseCase, UseCaseEvent, UseCaseState};
use crate::utils::ExponentialBackoff;

/// Overlay Discovery Configuration.
///
/// Simple data struct.
#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub struct ONDConfig {
    /// Default minimal [Duration] between subsequent sent FindNodeReq.
    pub send_timeout: Duration,
    /// Number of contacts in the overlay Neighborhood to include in the FindNodeReq
    /// and to be returned by the FindNodeRsp.
    pub overlay_neighborhood_size: NonZeroU64,
    /// Base number for the backoff computation.
    ///
    /// Defaults to **2**.
    pub backoff_base: NonZeroU32,
    /// Maximum number of subsequent retries to send a FindNodeReq.
    ///
    /// Defaults to **6**.
    pub backoff_max_retries: NonZeroU32,
    /// Initial [Duration] for the backoff which will exponentially increased.
    ///
    /// Defaults to *250ms*.
    pub backoff_starting_duration: Duration,
    /// Number of grouped bits used for calculating the shared prefix of two [NodeId]s.
    pub shared_prefix_bits_grouping: NonZeroUsize,
}

impl Default for ONDConfig {
    fn default() -> Self {
        Self {
            // TODO: Useful timeout duration?
            send_timeout: Duration::from_secs(5),
            overlay_neighborhood_size: NonZeroU64::new(20).unwrap(),
            backoff_base: NonZeroU32::new(2).unwrap(),
            backoff_max_retries: NonZeroU32::new(6).unwrap(),
            backoff_starting_duration: Duration::from_micros(250),
            shared_prefix_bits_grouping: NonZeroUsize::new(1).unwrap(),
        }
    }
}

/// State of the Overlay Neighborhood Discovery (OND) use case.
#[derive(Debug, Eq, PartialEq, Clone)]
pub enum ONDState {
    /// OND is initialized but not running yet.
    ///
    /// Transition to [ONDiscState::Running] by calling [OverlayNeighborhoodDiscovery::start].
    Initialized,
    /// OND was started and is running.
    ///
    /// Holds the currently active timer, already sent FindNodeReq [Nonce]s and the [Nonce]
    /// of the latest sent FindNodeReq.
    Running {
        /// The [TimerId] of the currently active timer for this use case.
        timer_id: TimerId,
        /// Contains all messages [Nonce]s including the latest.
        nonces: HashSet<Nonce>,
        /// Contains the latest FindNodeReqs [Nonce] if any was sent yet.
        latest: Option<Nonce>,
    },
    /// The use case encountered an unrecoverable error.
    Error,
}

impl UseCaseState for ONDState {
    fn is_finished(&self) -> bool {
        false
    }

    fn is_error(&self) -> bool {
        self == &Self::Error
    }
}

/// Overlay Neighborhood Discovery (OND) use case.
///
/// Implements the periodic process of sending [ProtocolMessage::FindNodeReq]
/// with exponential backoff on error.
///
/// # Timing
///
/// A new FindNodeReq is scheduled to be sent after [ONDConfig::send_timeout]
/// after the retrieval of an answer to the last FindNodeReq or the exponential backoff failed.
///
/// # Nonces
///
/// The [Nonce] of retry messages are not equal to each other.
/// The [Nonce] of every sent FindNodeReq is randomly generated to distinguish
/// between answers to the latest and older FindNodeReqs.
#[derive(Debug, Clone)]
pub struct ONDUseCase<C, const BUCKET_SIZE: usize> {
    _pd: PhantomData<C>,
    state: ONDState,
    config: ONDConfig,
    backoff: ExponentialBackoff,
}

impl<C, const BUCKET_SIZE: usize> Default for ONDUseCase<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self::new(ONDConfig::default())
    }
}

impl<C, const BUCKET_SIZE: usize> ONDUseCase<C, BUCKET_SIZE> {
    /// Create a new [ONDUseCase] from an [ONDConfig].
    pub fn new(config: ONDConfig) -> Self {
        Self {
            _pd: PhantomData::default(),
            state: ONDState::Initialized,
            config,
            backoff: ExponentialBackoff::new(
                config.backoff_base.get(),
                config.backoff_max_retries.get(),
                config.backoff_starting_duration,
            ),
        }
    }
}

impl<C, const BUCKET_SIZE: usize> ONDUseCase<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::MessageSender: ProtocolMessageSender,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
{
    /// Sets a new timer accordingly and sends a new FindNodeReq
    /// if exponential backoff allows it.
    fn send_next_request(&mut self, context: &C) -> Result<(), <Self as UseCase>::Error> {
        let (timer_id, nonces, latest) = if let ONDState::Running {
            timer_id,
            nonces,
            latest,
        } = &mut self.state
        {
            (timer_id, nonces, latest)
        } else {
            panic!("Called handle_timer_event in non-Running state!");
        };

        // if no next backoff is allowed -> Restart whole process
        let next_backoff = self.backoff.next();
        if next_backoff.is_none() {
            log::warn!("Exponential Backoff for Overlay Neighborhood Discovery failed. Scheduling next iteration.");
            // Reset backoff
            self.backoff.reset();
            // Old nonces are useless now
            nonces.clear();
            *latest = None;

            // Schedule new send
            *timer_id = context.runtime().register_timer(self.config.send_timeout);
            return Ok(());
        };
        // otherwise send next message and register another timer
        let next_backoff = next_backoff.unwrap();

        let nonce = Nonce::random();
        nonces.insert(nonce.clone());
        *latest = Some(nonce.clone());

        // No need for discovery if isolated
        if context.pn_table().is_empty() {
            log::error!("No physical neighbors present; Node is isolated");
            return Ok(());
        }

        // Get the path to the closest node of ourselves
        let path_to_closest_on = context
            .routing_table()
            .get_closest(
                context.root_id(),
                self.config.shared_prefix_bits_grouping.get(),
            )
            .cloned();

        // No one found -> Isolated
        if path_to_closest_on.is_none() {
            log::debug!("Node has no valid contacts. Can't discover overlay neighbors");
            return Err(ONDError::NeighborInconsistency);
        }
        let contact = path_to_closest_on.unwrap();
        let route_to_closest_on = SourceRoute::from(contact.path().clone());

        // Get the port of the next physical neighbor to route this request through
        let neighbor = route_to_closest_on.current_hop();
        let port = context.pn_table().get(neighbor).cloned();
        if port.is_none() {
            log::error!("Temporary inconsistency. Contact is valid but its path goes through a invalid neighbor. Contact: {}, neighbor: {}", contact, neighbor);
            return Err(ONDError::NeighborInconsistency);
        }
        let port = port.unwrap();

        let request = ReqRspMessage {
            nonce,
            source: context.root_id().clone(),
            target: context.root_id().clone(),
            data: FindNodeReqData {
                exact: false,
                neighborhood: self.config.overlay_neighborhood_size,
            },
            source_route: route_to_closest_on,
        };

        if let Err(e) = context.message_sender_mut().send(request, port) {
            log::error!("Failed to send FindNodeReq: {:?}", e);
            self.state = ONDState::Error;
            return Err(ONDError::SendError);
        }

        // Start next backoff timer
        *timer_id = context.runtime().register_timer(next_backoff);

        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum ONDError {
    SendError,
    NeighborInconsistency,
}

impl Display for ONDError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SendError => write!(f, "Failed to send overlay neighbor discovery message"),
            Self::NeighborInconsistency => {
                write!(
                    f,
                    "Missing contact in routing or pn table; Is the node isolated?"
                )
            }
        }
    }
}

impl Error for ONDError {}

impl<C, const BUCKET_SIZE: usize> UseCase for ONDUseCase<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    C::MessageSender: ProtocolMessageSender,
    // Isn't used yet, but provides information for the compiler to derive the BUCKET_SIZE from RT
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
{
    type Context = C;
    type Error = ONDError;
    type State = ONDState;

    fn start(&mut self, context: &Self::Context) -> Result<(), Self::Error> {
        let timer_id = context.runtime().register_timer(self.config.send_timeout);

        self.state = ONDState::Running {
            timer_id,
            nonces: HashSet::with_capacity(self.config.backoff_max_retries.get() as usize),
            latest: None,
        };

        Ok(())
    }

    /// As this reacts to Errors, changes to the RoutingTable must occur before delegating
    /// the event to this method.
    fn handle_event(
        &mut self,
        context: &Self::Context,
        event: UseCaseEvent,
    ) -> Result<(), Self::Error> {
        match (&mut self.state, event) {
            // Regular timer went off
            (ONDState::Running { timer_id, .. }, UseCaseEvent::Timer(received_timer_id)) => {
                if timer_id == &received_timer_id {
                    self.send_next_request(context)?;
                }
            }
            // A successful response was received
            (
                ONDState::Running {
                    timer_id,
                    nonces,
                    latest,
                },
                UseCaseEvent::Message(ProtocolMessage::FindNodeRsp(ReqRspMessage { nonce, .. }), _),
            ) => {
                if nonces.contains(&nonce) {
                    // Handling the data in the response is up to another use-case TODO
                    log::debug!("Overlay Neighborhood Discovery was successful!");

                    nonces.clear();
                    *latest = None;
                    self.backoff.reset();

                    // Schedule new send
                    *timer_id = context.runtime().register_timer(self.config.send_timeout);
                }
            }
            // An error response was received
            (
                ONDState::Running { latest, .. },
                UseCaseEvent::Message(ProtocolMessage::Error(ReqRspMessage { nonce, data, .. }), _),
            ) => {
                // This way errors to previous messages will be ignored, as they are
                // already interpreted as failed
                if latest == &Some(nonce) {
                    log::error!("Received error response for latest FindNodeReq: {:?}", data);

                    self.send_next_request(context)?;
                }
            }
            _ => {}
        }

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

#[cfg(all(test, feature = "bus"))]
mod tests {
    use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
    use std::time::Duration;

    use crate::broadcaster::BusBroadcaster;
    use crate::context::SyncContext;
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::domain::{
        Age, Contact, InsertionStrategyResult, NodeId, PNTable, Path, Port, RoutingTable,
        StateSeqNr, TestInsertionStrategy,
    };
    use crate::messaging::source_route::SourceRoute;
    use crate::messaging::tests::ArcSyncInMemoryMessageHub;
    use crate::messaging::{
        ErrorData, FindNodeReqData, InMemoryMessageHub, ProtocolMessage, ProtocolMessageReceiver,
        ReqRspMessage,
    };
    use crate::runtime::ImmediateRuntime;
    use crate::use_cases::overlay_neighborhood_discovery::{ONDConfig, ONDUseCase};
    use crate::use_cases::{UseCase, UseCaseEvent};

    #[test]
    fn starts_with_provided_configuration() {
        crate::tests::init();

        let config = ONDConfig {
            send_timeout: Duration::from_micros(5),
            overlay_neighborhood_size: NonZeroU64::new(1).unwrap(),
            ..Default::default()
        };

        let root_id = NodeId::random();

        let broadcaster = BusBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let sync_context = SyncContext::new(
            root_id.clone(),
            SingleBucketRT::<1>::new(root_id),
            PNTable::new(),
            TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            InMemoryMessageHub::new(),
            runtime.clone(),
        );

        let mut use_case = ONDUseCase::new(config);

        assert_eq!(use_case.start(&sync_context), Ok(()));
        // Dummy Runtime should emit the timer event immediately
        let timer_id = runtime.latest_timer_id();
        assert!(timer_id.is_some(), "No Timer was started after start");
        let timer_id = timer_id.unwrap();
        assert_eq!(
            runtime.get_timer_duration(&timer_id),
            Some(config.send_timeout)
        );
    }

    #[test]
    fn sends_find_node_on_timer_event() {
        crate::tests::init();

        let config = ONDConfig {
            send_timeout: Duration::from_micros(5),
            overlay_neighborhood_size: NonZeroU64::new(1).unwrap(),
            ..Default::default()
        };

        let root_id = NodeId::random();

        let broadcaster = BusBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let mut hub = ArcSyncInMemoryMessageHub::new();

        // At least one has contact has to be present and valid to forward message to
        // Otherwise the use case thinks the node is isolated
        let neighbor_id = NodeId::random();
        let mut routing_table = SingleBucketRT::<1>::new(root_id.clone());
        assert!(routing_table
            .insert(Contact::new(
                Path::from(neighbor_id.clone()),
                Age::from(0),
                StateSeqNr::from(0)
            ))
            .is_ok());
        let mut pn_table = PNTable::new();
        pn_table.add(neighbor_id, Port::Named(String::from("test")));

        let sync_context = SyncContext::new(
            root_id.clone(),
            routing_table,
            pn_table,
            TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            hub.clone(),
            runtime.clone(),
        );

        let mut use_case = ONDUseCase::new(config);

        assert_eq!(use_case.start(&sync_context), Ok(()));

        let timer_id = runtime.latest_timer_id();
        assert!(timer_id.is_some(), "A Timer has to be set");
        let timer_id = timer_id.unwrap();

        assert_eq!(
            use_case.handle_event(&sync_context, UseCaseEvent::Timer(timer_id)),
            Ok(())
        );

        let request = hub.recv_timeout(Some(Duration::from_secs(1)));
        assert!(
            request.is_ok(),
            "Error receiving Overlay Neighbor Discovery Output Request: {:?}",
            request
        );
        let request = request.unwrap();
        assert!(
            request.is_some(),
            "Message hub should return protocol message but returned: {:?}",
            request
        );
        let (message, _) = request.unwrap();
        if let ProtocolMessage::FindNodeReq(ReqRspMessage {
            target,
            source,
            data: FindNodeReqData { exact, .. },
            ..
        }) = message
        {
            assert!(!exact);
            assert_eq!(target, root_id);
            assert_eq!(source, root_id);
        } else {
            panic!(
                "Emitted ProtocolMessage is not the one expected: {:?}",
                message
            );
        }
    }

    #[test]
    fn exponential_backoff_through_timers() {
        crate::tests::init();

        let config = ONDConfig {
            send_timeout: Duration::from_secs(10),
            overlay_neighborhood_size: NonZeroU64::new(1).unwrap(),
            backoff_base: NonZeroU32::new(2).unwrap(),
            backoff_max_retries: NonZeroU32::new(3).unwrap(),
            backoff_starting_duration: Duration::from_micros(250),
            shared_prefix_bits_grouping: NonZeroUsize::new(1).unwrap(),
        };

        let root_id = NodeId::random();

        let broadcaster = BusBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let hub = ArcSyncInMemoryMessageHub::new();

        // At least one has contact has to be present and valid to forward message to
        // Otherwise the use case thinks the node is isolated
        let neighbor_id = NodeId::random();
        let mut routing_table = SingleBucketRT::<1>::new(root_id.clone());
        assert!(routing_table
            .insert(Contact::new(
                Path::from(neighbor_id.clone()),
                Age::from(0),
                StateSeqNr::from(0)
            ))
            .is_ok());
        let mut pn_table = PNTable::new();
        pn_table.add(neighbor_id, Port::Named(String::from("test")));

        let sync_context = SyncContext::new(
            root_id.clone(),
            routing_table,
            pn_table,
            TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            hub,
            runtime.clone(),
        );

        let mut use_case = ONDUseCase::new(config);

        assert_eq!(use_case.start(&sync_context), Ok(()));

        let timer_id = runtime
            .latest_timer_id()
            .expect("a new timer has to be scheduled");
        assert_eq!(
            use_case.handle_event(&sync_context, UseCaseEvent::Timer(timer_id)),
            Ok(())
        );
        let timer_id = runtime
            .latest_timer_id()
            .expect("a new timer has to be scheduled");
        // After triggering the first message, Exponential Backoff should start counting
        assert_eq!(
            runtime.get_timer_duration(&timer_id),
            Some(Duration::from_micros(250))
        );
        assert_eq!(
            use_case.handle_event(&sync_context, UseCaseEvent::Timer(timer_id)),
            Ok(())
        );

        let timer_id = runtime
            .latest_timer_id()
            .expect("a new timer has to be scheduled");
        assert_eq!(
            runtime.get_timer_duration(&timer_id),
            Some(Duration::from_micros(500))
        );
        assert_eq!(
            use_case.handle_event(&sync_context, UseCaseEvent::Timer(timer_id)),
            Ok(())
        );

        // Overflow should trigger redo
        let timer_id = runtime
            .latest_timer_id()
            .expect("a new timer has to be scheduled");
        assert_eq!(
            runtime.get_timer_duration(&timer_id),
            Some(Duration::from_micros(1000))
        );
        assert_eq!(
            use_case.handle_event(&sync_context, UseCaseEvent::Timer(timer_id)),
            Ok(())
        );
        let timer_id = runtime
            .latest_timer_id()
            .expect("a new timer has to be scheduled");
        assert_eq!(
            runtime.get_timer_duration(&timer_id),
            Some(Duration::from_secs(10))
        );
    }

    #[test]
    fn exponential_backoff_through_errors() {
        crate::tests::init();

        let config = ONDConfig {
            send_timeout: Duration::from_secs(10),
            overlay_neighborhood_size: NonZeroU64::new(1).unwrap(),
            backoff_base: NonZeroU32::new(2).unwrap(),
            backoff_max_retries: NonZeroU32::new(3).unwrap(),
            backoff_starting_duration: Duration::from_micros(250),
            shared_prefix_bits_grouping: NonZeroUsize::new(1).unwrap(),
        };

        let root_id = NodeId::random();

        let broadcaster = BusBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let mut hub = ArcSyncInMemoryMessageHub::new();

        // At least one has contact has to be present and valid to forward message to
        // Otherwise the use case thinks the node is isolated
        let neighbor_id = NodeId::random();
        let mut routing_table = SingleBucketRT::<1>::new(root_id.clone());
        assert!(routing_table
            .insert(Contact::new(
                Path::from(neighbor_id.clone()),
                Age::from(0),
                StateSeqNr::from(0)
            ))
            .is_ok());
        let mut pn_table = PNTable::new();
        pn_table.add(neighbor_id, Port::Named(String::from("test")));

        let sync_context = SyncContext::new(
            root_id.clone(),
            routing_table,
            pn_table,
            TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            hub.clone(),
            runtime.clone(),
        );

        let mut use_case = ONDUseCase::new(config);

        assert_eq!(use_case.start(&sync_context), Ok(()));

        // Trigger first Send.
        let timer_id = runtime
            .latest_timer_id()
            .expect("a new timer has to be scheduled");
        assert_eq!(
            use_case.handle_event(&sync_context, UseCaseEvent::Timer(timer_id)),
            Ok(())
        );
        let timer_id = runtime
            .latest_timer_id()
            .expect("a new timer has to be scheduled");
        assert_eq!(
            runtime.get_timer_duration(&timer_id),
            Some(Duration::from_micros(250))
        );

        let (request, _) = hub
            .recv_timeout(Some(Duration::from_secs(1)))
            .expect("Should not emit error")
            .expect("should return actual message");
        let (nonce, source_route) = if let ProtocolMessage::FindNodeReq(req_rsp_message) = &request
        {
            (
                req_rsp_message.nonce.clone(),
                req_rsp_message.source_route.clone(),
            )
        } else {
            panic!("No FindNodeReq sent: {:?}", request);
        };

        let error_node_id = NodeId::random();

        let error = UseCaseEvent::Message(
            ProtocolMessage::Error(ReqRspMessage {
                nonce,
                source: error_node_id.clone(),
                target: root_id.clone(),
                data: ErrorData::DeadEnd,
                source_route: SourceRoute::from_reversed(source_route),
            }),
            InMemoryMessageHub::dummy_port(),
        );
        assert_eq!(use_case.handle_event(&sync_context, error), Ok(()));
        let timer_id = runtime
            .latest_timer_id()
            .expect("a timer should be created");
        assert_eq!(
            runtime.get_timer_duration(&timer_id),
            Some(Duration::from_micros(500))
        );

        let (request, _) = hub
            .recv_timeout(Some(Duration::from_secs(1)))
            .expect("Should not emit error")
            .expect("should return actual message");
        let (nonce, source_route) = if let ProtocolMessage::FindNodeReq(req_rsp_message) = &request
        {
            (
                req_rsp_message.nonce.clone(),
                req_rsp_message.source_route.clone(),
            )
        } else {
            panic!("No FindNodeReq sent: {:?}", request);
        };

        let error_node_id = NodeId::random();

        let error = UseCaseEvent::Message(
            ProtocolMessage::Error(ReqRspMessage {
                nonce,
                source: error_node_id.clone(),
                target: root_id.clone(),
                data: ErrorData::DeadEnd,
                source_route: SourceRoute::from_reversed(source_route),
            }),
            InMemoryMessageHub::dummy_port(),
        );
        assert_eq!(use_case.handle_event(&sync_context, error), Ok(()));
        let timer_id = runtime
            .latest_timer_id()
            .expect("a timer should be created");
        assert_eq!(
            runtime.get_timer_duration(&timer_id),
            Some(Duration::from_micros(1000))
        );

        // Overflow should trigger redo
        let (request, _) = hub
            .recv_timeout(Some(Duration::from_secs(1)))
            .expect("Should not emit error")
            .expect("should return actual message");
        let (nonce, source_route) = if let ProtocolMessage::FindNodeReq(req_rsp_message) = &request
        {
            (
                req_rsp_message.nonce.clone(),
                req_rsp_message.source_route.clone(),
            )
        } else {
            panic!("No FindNodeReq sent: {:?}", request);
        };

        let error_node_id = NodeId::random();

        let error = UseCaseEvent::Message(
            ProtocolMessage::Error(ReqRspMessage {
                nonce,
                source: error_node_id.clone(),
                target: root_id.clone(),
                data: ErrorData::DeadEnd,
                source_route: SourceRoute::from_reversed(source_route),
            }),
            InMemoryMessageHub::dummy_port(),
        );
        assert_eq!(use_case.handle_event(&sync_context, error), Ok(()));
        let timer_id = runtime
            .latest_timer_id()
            .expect("a timer should be created");
        assert_eq!(
            runtime.get_timer_duration(&timer_id),
            Some(Duration::from_secs(10))
        );
    }
}
