use std::collections::HashMap;
use std::fmt::Debug;
use std::marker::PhantomData;
use std::num::{NonZeroU8, NonZeroU32, NonZeroU64};
use std::ops::Deref;
use std::time::Duration;
use tracing::{Level, instrument};

use derive_more::Display;

use crate::domain::{GroupingError, NodeId, RoutingTable, ULNTable, UnderlayNeighborId};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{
    CommonHeader, FindNodeReqData, Nonce, ProtocolMessage, ProtocolMessageKind, ReqRspMessage,
};
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{
    EventHandler, TimerId, UseCase, UseCaseContext, UseCaseEvent, UseCaseState, VicinityEvent,
};
use crate::utils::ExponentialBackoff;

/// Overlay Discovery Configuration.
///
/// Simple data struct.
#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub struct ONDConfig<const BUCKET_SIZE: usize> {
    /// Default minimal [Duration] between subsequent sent FindNodeReq.
    pub send_timeout: Duration,
    /// Default [Duration] between subsequent sent FindNodeReq for random exploration.
    pub random_exploration_interval: Duration,
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

impl<const BUCKET_SIZE: usize> Default for ONDConfig<BUCKET_SIZE> {
    fn default() -> Self {
        Self {
            // TODO: Useful timeout duration?
            send_timeout: Duration::from_millis(250),
            random_exploration_interval: Duration::from_millis(400),
            overlay_neighborhood_size: NonZeroU64::new(BUCKET_SIZE.try_into().unwrap()).unwrap(),
            backoff_base: NonZeroU32::new(2).unwrap(),
            backoff_max_retries: NonZeroU32::new(10).unwrap(), // 0,25s*1024=256s = 4min 16s
            backoff_starting_duration: Duration::from_millis(250),
            shared_prefix_bits_grouping: NonZeroU8::MIN,
        }
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
#[repr(u8)]
pub enum TimerType {
    PeriodicJoin,
    RandomExploration,
    Max,
}

#[derive(Debug, Eq, PartialEq, Clone, Default, Copy)]
pub struct TimerContext {
    timer_id: Option<TimerId>,
    backoff: Option<ExponentialBackoff>,
    last_msg_state: Option<(Nonce, NodeId)>,
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
    /// Holds the currently active timers and the [Nonce]
    /// of the latest sent FindNodeReq.
    Running {
        /// for handling different timers
        /// Contains the latest FindNodeReq [Nonce] and [NodeId] if any was sent yet
        /// [ExponentialBackoff] is not used for every timer
        ///
        timer_contexts: [TimerContext; TimerType::Max as usize],
    },
    /// Node is currently isolated and pauses sending messages
    Isolated,
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
/// with exponential backoff and final limit
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
    config: ONDConfig<BUCKET_SIZE>,
}

impl<C, const BUCKET_SIZE: usize> Default for OverlayNeighborhoodDiscovery<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self::new(ONDConfig::default()).expect("default shared prefix should be valid")
    }
}

impl<C, const BUCKET_SIZE: usize> OverlayNeighborhoodDiscovery<C, BUCKET_SIZE> {
    /// Create a new [OverlayNeighborhoodDiscovery] from an [ONDConfig].
    pub fn new(config: ONDConfig<BUCKET_SIZE>) -> Result<Self, GroupingError> {
        if config.shared_prefix_bits_grouping.get() > NodeId::BITS {
            return Err(GroupingError::Invalid {
                group_size: config.shared_prefix_bits_grouping,
            });
        }

        Ok(Self {
            _pd: PhantomData,
            state: ONDState::Initialized,
            config,
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
    /// Sets a new timer accordingly and sends a new FindNodeReq with target of our own ID (this is a join)
    /// if exponential backoff allows it.
    fn send_next_join_request(&mut self, context: &C) -> Result<(), <Self as EventHandler>::Error> {
        let current_timer_context = if let ONDState::Running { timer_contexts } = &mut self.state {
            &mut timer_contexts[TimerType::PeriodicJoin as usize]
        } else {
            panic!("Called send_next_join_request in non-running state!");
        };

        // No need for discovery if currently isolated
        if context.uln_table().is_empty() {
            log::warn!(target: "overlay_neighborhood_discovery",
                "No underlay neighbors present; Node is isolated"
            );
            // change to error state
            // the use case will change to running again if not isolated anymore
            self.state = ONDState::Isolated;
            return Ok(());
        }

        // the initial timer has no exponential backoff (and no state)
        // then a randomized exponential backoff timer is created
        if current_timer_context.backoff.is_none() {
            // create exponential backoff
            let mut backoff = ExponentialBackoff::random_with_default_base(
                self.config.backoff_max_retries.get(),
                self.config.backoff_starting_duration,
            );
            // the timer will never end
            backoff.set_limit();
            // either update existing entry or insert new timer into timer_contexts, nonce and node ID are updated later.
            current_timer_context.backoff = Some(backoff);
            log::trace!(
                target: "overlay_neighborhood_discovery",
                "setting new backoff: {backoff:?}, timer_context: {current_timer_context:?}"
            );
        };

        let next_duration = current_timer_context
            .backoff
            .as_mut() // we need to update the backoff in the stored timer_context!
            .unwrap()
            .next()
            .expect("Periodic join timer is always expected to have a value");

        // check if last message has been answered
        let avoid_contact = if let Some((msg_id, node_id)) = current_timer_context.last_msg_state {
            // message has not seen a reply (neither successful nor error)
            log::info!(
                target: "overlay_neighborhood_discovery",
                "Did not received a response for FindNodeReq (JOIN) with msg_id: {msg_id} in time"
            );
            Some(node_id)
        } else {
            None
        };

        let new_nonce = Nonce::random();

        let closest_via_contacts = context
            .routing_table()
            .closest(
                context.root_id(),
                3,
                self.config.shared_prefix_bits_grouping,
            )
            .expect("config should have been checked before");

        // Get the path to the closest node of ourselves
        // in case the previous FindNodeReq failed, we try to avoid that contact
        let closest_on = if avoid_contact.is_some() {
            closest_via_contacts
                .iter()
                .find(|(_, contact)| (*contact).id() != &avoid_contact.unwrap())
                .map(|(_, contact)| (*contact).clone())
        } else {
            closest_via_contacts
                .first()
                .map(|(_, contact)| (*contact).clone())
        };

        // No one found -> overlay-wise isolated, but underlay neighbors are present
        // this should never happen as the underlay neighbor are then the closest overlay neighbors
        if closest_on.is_none() {
            log::warn!(
                target: "overlay_neighborhood_discovery",
                "Underlay neighbors are present, but no usable contacts; overlay probably not connected. RT: {:?}, NT: {:?}",
                *context.routing_table(),
                *context.uln_table()
            );
            return Ok(());
        }
        let contact = closest_on.unwrap();
        let contact_via_id = *contact.id();
        let mut route_to_closest_on = SourceRoute::from(contact.path().unwrap().clone());
        route_to_closest_on.push_front(*context.root_id());

        // Get the interface of the next underlay neighbor to route this request through
        let neighbor = route_to_closest_on.current_hop();
        let interface = context.uln_table().get(neighbor).cloned();
        if interface.is_none() {
            log::error!(
                target: "overlay_neighborhood_discovery",
                "Contact is valid but its path goes through an invalid neighbor. Contact: {contact}, neighbor: {neighbor}"
            );
            return Ok(());
        }

        // compose FindNodeRequest to our own ID (Join)
        let request = ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::FindNodeReq,
                *context.root_id(),
                *contact.id(),
                Some(new_nonce.into()),
                Some(From::from(*context.uln_table().state_seq_nr())),
                context.uln_table().size(),
            ),
            data: FindNodeReqData {
                exact: false,
                neighborhood: self.config.overlay_neighborhood_size,
                target: *context.root_id(), // own ID as target
            },
            not_via: None,
            source_route: route_to_closest_on,
        };

        log::trace!(
            target: "overlay_neighborhood_discovery",
            "Sending FindNodeReq (Join) message (msg_id={new_nonce}) {request:?}"
        );

        context
            .runtime()
            .send_message(request, context.uln_table().deref(), context.root_id());

        log::trace!(
            target: "overlay_neighborhood_discovery",
            "Timer next duration: {next_duration:?}"
        );

        // Start next backoff timer
        current_timer_context.timer_id = Some(context.runtime().register_timer(next_duration));
        current_timer_context.last_msg_state = Some((new_nonce, contact_via_id));

        Ok(())
    }

    /// Sends a new FindNodeReq with random target to explore the ID space for new and better contacts and routes
    fn send_next_request_to_random_id(
        &mut self,
        context: &C,
    ) -> Result<(), <Self as EventHandler>::Error> {
        let current_timer_context = if let ONDState::Running { timer_contexts } = &mut self.state {
            &mut timer_contexts[TimerType::RandomExploration as usize]
        } else {
            panic!("Called send_next_request_to_random_id in non-running state!");
        };

        // No need for randomized exploration if currently isolated
        if context.uln_table().is_empty() {
            log::warn!(target: "overlay_neighborhood_discovery",
                       "No underlay neighbors present; Node is isolated"
            );
            // change to error state
            // the use case will change to running again if not isolated anymore
            self.state = ONDState::Isolated;
            return Ok(());
        }

        let new_nonce = Nonce::random();
        let random_id = NodeId::random();

        let closest_contact_path = context
            .routing_table()
            .closest(&random_id, 1, self.config.shared_prefix_bits_grouping)
            .expect("grouping was checked on initialization")
            .first() // TODO: Proximity Neighbor Selection
            .map(|(_, contact)| contact.path().unwrap()) // closest returns only valid contacts
            .cloned();

        // No one found -> overlay-wise isolated, but underlay neighbors are present
        // this should never happen as the underlay neighbor are then the closest overlay neighbors
        if closest_contact_path.is_none() {
            log::warn!(
                target: "overlay_neighborhood_discovery",
                "Underlay neighbors are present, but no usable contacts; overlay probably not connected. RT: {:?}, NT: {:?}",
                *context.routing_table(),
                *context.uln_table()
            );
            return Ok(());
        }

        let closest_path = closest_contact_path.unwrap();

        // Get interface of neighbor
        let neighbor = closest_path.first();
        let interface = context.uln_table().get(neighbor).cloned();
        if interface.is_none() {
            log::error!(
                target: "overlay_neighborhood_discovery",
                "Contact is valid but its path goes through an invalid neighbor. Contact: {}, neighbor: {}",
                closest_path.last(),
                neighbor
            );
            return Ok(());
        }

        let mut route = SourceRoute::from(closest_path);
        route.push_front(*context.root_id());

        let message = ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::FindNodeReq,
                *context.root_id(),
                *route.destination(),
                Some(new_nonce.into()),
                Some(From::from(*context.uln_table().state_seq_nr())),
                context.uln_table().size(),
            ),
            data: FindNodeReqData {
                exact: false, // the closest node to random ID should reply
                neighborhood: self.config.overlay_neighborhood_size,
                target: random_id, //random ID
            },
            not_via: None,
            source_route: route,
        };
        log::trace!(target: "overlay_neighborhood_discovery", "Sending message for random exploration {message:?}");

        context
            .runtime()
            .send_message(message, context.uln_table().deref(), context.root_id());

        current_timer_context.last_msg_state = Some((new_nonce, random_id));
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
        self.state = ONDState::Running {
            timer_contexts: [TimerContext {
                timer_id: None,
                backoff: None,
                last_msg_state: None,
            }; TimerType::Max as usize],
        };

        // the first messages should be sent after a small delay because this
        // node needs to learn its underlay vicinity first, before it can send
        // something meaningful to find its overlay neighbors
        let join_timer_id = context.runtime().register_timer(self.config.send_timeout);
        let random_exploration_timer_id = context
            .runtime()
            .register_periodic_timer(self.config.random_exploration_interval);
        if let ONDState::Running { timer_contexts, .. } = &mut self.state {
            // remember the just registered initial timer, however, timer_context is None
            timer_contexts[TimerType::PeriodicJoin as usize].timer_id = Some(join_timer_id);
            timer_contexts[TimerType::RandomExploration as usize].timer_id =
                Some(random_exploration_timer_id);
        } else {
            panic!("State must not have changed from ONDState::Running!");
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
            (ONDState::Running { timer_contexts, .. }, UseCaseEvent::Timer(received_timer_id)) => {
                // need to check if this timer belongs to this use case
                match received_timer_id {
                    join_timer_id
                        if Some(join_timer_id)
                            == timer_contexts[TimerType::PeriodicJoin as usize].timer_id =>
                    {
                        self.send_next_join_request(context)?;
                    }
                    rand_exploration_timer_id
                        if Some(rand_exploration_timer_id)
                            == timer_contexts[TimerType::RandomExploration as usize].timer_id =>
                    {
                        self.send_next_request_to_random_id(context)?;
                    }
                    _ => {
                        // maybe a timeout for another use case, so simply ignore here
                        return Ok(());
                    }
                }
            }
            // A successful response was received
            (
                ONDState::Running { timer_contexts },
                UseCaseEvent::Message(
                    ProtocolMessage::FindNodeRsp(ReqRspMessage { common_header, .. }),
                    _,
                ),
            ) => {
                let recvd_nonce = Nonce::from(common_header.msg_id());
                // need to extract latest nonce from timer state
                // currently we do not care about tracking responses to random exploration messages
                if let Some((latest_nonce, _)) =
                    timer_contexts[TimerType::PeriodicJoin as usize].last_msg_state
                    && latest_nonce == recvd_nonce
                {
                    log::debug!(
                        target: "overlay_neighborhood_discovery",
                        "Received successful response for latest FindNodeReq (Join): msg_id={} src={}",
                        recvd_nonce,
                        common_header.src_node_id(),
                    );
                    // delete last msg state
                    timer_contexts[TimerType::PeriodicJoin as usize].last_msg_state = None;
                    // nothing else to do, because timer will trigger sending of the next join message
                }
                // TODO react on random exploration response: estimate number of nodes in the network and
                // get statistics about failed requests (may be a hint about network stability)
            }
            // An error response was received
            (
                ONDState::Running { timer_contexts },
                UseCaseEvent::Message(
                    ProtocolMessage::Error(ReqRspMessage {
                        common_header,
                        data,
                        ..
                    }),
                    _,
                ),
            ) => {
                let recvd_nonce = Nonce::from(common_header.msg_id());
                // need to extract latest nonce from timer state
                // currently we do not care about responses to random exploration messages
                if let Some((latest_nonce, _)) =
                    timer_contexts[TimerType::PeriodicJoin as usize].last_msg_state
                    && latest_nonce == recvd_nonce
                {
                    log::warn!(
                    target: "overlay_neighborhood_discovery",
                    "Received error response for latest FindNodeReq (Join): {data:?}"
                    );
                    // delete last msg state
                    timer_contexts[TimerType::PeriodicJoin as usize].last_msg_state = None;
                    // no need for further actions, one can assume that a rediscovery will repair the contact
                    // before the next join is sent
                }
            }
            // check for restarting use case after isolation
            (ONDState::Isolated, UseCaseEvent::Vicinity(vicinity_event)) => {
                //
                if vicinity_event == VicinityEvent::Changed && !context.uln_table().is_empty() {
                    let _ignore = self.start(context);
                } else {
                    return Ok(());
                }
            }
            // TODO: send FindNodeReq if new Contact inserted in last bucket of RoutingTable: src/routing/r2kademlia/R2KademliaPolicyHandlers.cc:204
            // TODO: randomly probe for new path to contact with a FindNodeVia src/routing/r2kademlia/R2KademliaPeriodicTasks.cc:321
            _ => {}
        }

        Ok(())
    }
}
