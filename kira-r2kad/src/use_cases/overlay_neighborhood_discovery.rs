use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::marker::PhantomData;
use std::num::{NonZeroU8, NonZeroU32, NonZeroU64};
use std::ops::Deref;
use std::time::Duration;
use tracing::{Level, instrument};

use derive_more::Display;

use crate::domain::{GroupingError, NodeId, NotVia, RoutingTable, ULNTable, UnderlayNeighborId};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{
    CommonHeader, FindNodeReqData, Nonce, ProtocolMessage, ProtocolMessageKind, ReqRspMessage,
};
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{
    EventHandler, TimerId, UseCase, UseCaseContext, UseCaseEvent, UseCaseState,
};
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
    /// Number of grouped bits used for calculating the shared prefix of two [NodeIds](crate::domain::NodeId).
    pub shared_prefix_bits_grouping: NonZeroU8,
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
            shared_prefix_bits_grouping: NonZeroU8::MIN,
        }
    }
}

/// State of the Overlay Neighborhood Discovery (OND) use case.
#[derive(Debug, Eq, PartialEq, Clone)]
pub enum ONDState {
    /// OND is initialized but not running yet.
    ///
    /// Transition to [ONDState::Running] by calling [OverlayNeighborhoodDiscovery::start].
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
/// This should prevent multiple discoveries to overlap because of timeout configuration.
///
/// # Nonces
///
/// The [Nonce] of retry messages are not equal to each other.
/// The [Nonce] of every sent FindNodeReq is randomly generated to distinguish
/// between answers to the latest and older FindNodeReqs.
///
/// # Errors
///
/// Inconsistencies (underlay neighbors without contacts, contacts with invalid paths)
/// yield an error and will change the state to an unrecoverable error state.
/// The reason is that a failing neighbor and its removal should yield changes to un_table
/// and routing table at the same time.
#[derive(Debug, Clone)]
pub struct OverlayNeighborhoodDiscovery<C, const BUCKET_SIZE: usize> {
    _pd: PhantomData<C>,
    state: ONDState,
    config: ONDConfig,
    backoff: ExponentialBackoff,
}

impl<C, const BUCKET_SIZE: usize> Default for OverlayNeighborhoodDiscovery<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self::new(ONDConfig::default()).expect("default shared prefix should be valid")
    }
}

impl<C, const BUCKET_SIZE: usize> OverlayNeighborhoodDiscovery<C, BUCKET_SIZE> {
    /// Create a new [OverlayNeighborhoodDiscovery] from an [ONDConfig].
    pub fn new(config: ONDConfig) -> Result<Self, GroupingError> {
        if config.shared_prefix_bits_grouping.get() > NodeId::BITS {
            return Err(GroupingError::Invalid {
                group_size: config.shared_prefix_bits_grouping,
            });
        }

        Ok(Self {
            _pd: PhantomData,
            state: ONDState::Initialized,
            config,
            backoff: ExponentialBackoff::new(
                config.backoff_base.get(),
                config.backoff_max_retries.get(),
                config.backoff_starting_duration,
            ),
        })
    }
}

impl<C, const BUCKET_SIZE: usize> OverlayNeighborhoodDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE> + Debug,
    C::UnderlayNeighborTable:
        ULNTable + Debug + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    /// Sets a new timer accordingly and sends a new FindNodeReq
    /// if exponential backoff allows it.
    fn send_next_request(&mut self, context: &C) -> Result<(), <Self as EventHandler>::Error> {
        let (timer_id, nonces, latest) = if let ONDState::Running {
            timer_id,
            nonces,
            latest,
        } = &mut self.state
        {
            (timer_id, nonces, latest)
        } else {
            panic!("Called send_next_request in non-Running state!");
        };

        // if no next backoff is allowed -> Restart whole process
        let next_backoff = self.backoff.next();
        if next_backoff.is_none() {
            log::trace!(
                target: "overlay_neighborhood_discovery",
                "Exponential Backoff failed. Scheduling next iteration"
            );
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
        nonces.insert(nonce);
        *latest = Some(nonce);

        // No need for discovery if isolated
        if context.uln_table().is_empty() {
            log::warn!(target: "overlay_neighborhood_discovery",
                "No underlay neighbors present; Node is isolated"
            );
            return Ok(());
        }

        // Get the path to the closest node of ourselves
        let path_to_closest_on = context
            .routing_table()
            .closest(
                context.root_id(),
                1,
                self.config.shared_prefix_bits_grouping,
            )
            .expect("config should have been checked before")
            .first()
            .map(|(_, contact)| (*contact).clone());

        // No one found -> Isolated, but underlay neighbors are present
        if path_to_closest_on.is_none() {
            log::warn!(
                target: "overlay_neighborhood_discovery",
                "Underlay neighbors are present, but no contacts; Assuming isolation due to invalid neighbors. RT: {:?}, NT: {:?}",
                *context.routing_table(),
                *context.uln_table()
            );
            return Err(ONDError::NeighborInconsistency);
        }
        let contact = path_to_closest_on.unwrap();
        let mut route_to_closest_on = SourceRoute::from(contact.path().clone());
        route_to_closest_on.push_front(*context.root_id());

        // Get the interface of the next underlay neighbor to route this request through
        let neighbor = route_to_closest_on.current_hop();
        let interface = context.uln_table().get(neighbor).cloned();
        if interface.is_none() {
            log::error!(
                target: "overlay_neighborhood_discovery",
                "Contact is valid but its path goes through an invalid neighbor. Contact: {contact}, neighbor: {neighbor}"
            );
            self.state = ONDState::Error;
            return Err(ONDError::NeighborInconsistency);
        }

        let request = ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::FindNodeReq,
                *context.root_id(),
                *contact.id(),
                Some(nonce.into()),
                Some(From::from(*context.uln_table().state_seq_nr())),
                context.uln_table().size(),
            ),
            data: FindNodeReqData {
                exact: false,
                neighborhood: self.config.overlay_neighborhood_size,
                target: *context.root_id(),
            },
            not_via: context.not_via_state().iter().map(NotVia::from).collect(),
            source_route: route_to_closest_on,
        };

        log::trace!(
            target: "overlay_neighborhood_discovery",
            "Sending message {request:?}"
        );

        context
            .runtime()
            .send_message(request, context.uln_table().deref());

        // Start next backoff timer
        *timer_id = context.runtime().register_timer(next_backoff);

        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq, Clone, Display)]
pub enum ONDError {
    #[display("Missing contact in routing or ulntable; Is the node isolated?")]
    NeighborInconsistency,
}

impl<C, const BUCKET_SIZE: usize> UseCase for OverlayNeighborhoodDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    // Isn't used yet, but provides information for the compiler to derive the BUCKET_SIZE from RT
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE> + Debug,
    C::UnderlayNeighborTable:
        ULNTable + Debug + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    type State = ONDState;

    fn start(&mut self, context: &C) -> Result<(), Self::Error> {
        let timer_id = context.runtime().register_timer(self.config.send_timeout);

        self.state = ONDState::Running {
            timer_id,
            nonces: HashSet::with_capacity(self.config.backoff_max_retries.get() as usize),
            latest: None,
        };

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

impl<C, const BUCKET_SIZE: usize> EventHandler for OverlayNeighborhoodDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    // Isn't used yet, but provides information for the compiler to derive the BUCKET_SIZE from RT
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE> + Debug,
    C::UnderlayNeighborTable:
        ULNTable + Debug + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    type Context = C;
    type Error = ONDError;
    type Value = ();

    /// As this reacts to Errors, changes to the RoutingTable must occur before delegating
    /// the event to this method.
    #[instrument(
        level = Level::TRACE,
        target = "overlay_neighborhood_discovery",
        "overlay_neighborhood_discovery",
        skip(self, context),
        fields(
            state = ?self.state,
            config = ?self.config
        )
    )]
    fn handle_event(
        &mut self,
        context: &C,
        event: UseCaseEvent,
    ) -> Result<Self::Value, Self::Error> {
        match (&mut self.state, event) {
            // Regular timer went off
            (ONDState::Running { timer_id, .. }, UseCaseEvent::Timer(received_timer_id))
                if timer_id == &received_timer_id =>
            {
                self.send_next_request(context)?;
            }
            // A successful response was received
            (
                ONDState::Running {
                    timer_id,
                    nonces,
                    latest,
                },
                UseCaseEvent::Message(
                    ProtocolMessage::FindNodeRsp(ReqRspMessage { common_header, .. }),
                    _,
                ),
            ) => {
                let nonce = Nonce::from(common_header.msg_id());
                if nonces.contains(&nonce) {
                    // data will be handled in the forwarding UseCase
                    log::debug!(
                        target: "overlay_neighborhood_discovery",
                        "Overlay Neighborhood Discovery was successful!"
                    );

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
                UseCaseEvent::Message(
                    ProtocolMessage::Error(ReqRspMessage {
                        common_header,
                        data,
                        ..
                    }),
                    _,
                ),
            ) => {
                let nonce = Nonce::from(common_header.msg_id());
                // This way errors to previous messages will be ignored, as they are
                // already interpreted as failed
                if latest == &Some(nonce) {
                    log::warn!(
                        target: "overlay_neighborhood_discovery",
                        "Received error response for latest FindNodeReq: {data:?}"
                    );

                    self.send_next_request(context)?;
                }
            }
            // TODO: send FindNodeReq if new Contact inserted in last bucket of RoutingTable: src/routing/r2kademlia/R2KademliaPolicyHandlers.cc:204
            // TODO: randomly probe for new nodes: src/routing/r2kademlia/R2KademliaPeriodicTasks.cc:349
            // TODO: randomly probe for new path to contact with a FindeNodeVia src/routing/r2kademlia/R2KademliaPeriodicTasks.cc:321
            _ => {}
        }

        Ok(())
    }
}
