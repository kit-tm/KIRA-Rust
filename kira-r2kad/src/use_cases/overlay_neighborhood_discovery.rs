use std::{
    collections::HashMap,
    fmt::Debug,
    marker::PhantomData,
    num::{
        NonZeroU32,
        NonZeroU64,
    },
    ops::Deref,
    time::Duration,
};

use derive_more::Display;
use tracing::instrument;

use crate::{
    domain::{
        NodeId,
        Nonce,
        ProtocolMessage,
        ProtocolMessageKind,
        RoutingTable,
        SourceRoute,
        ULNState,
        ULNTable,
        UnderlayNeighborId,
        protocol_message::{
            CommonHeader,
            FindNodeReqData,
            ReqRspMessage,
        },
    },
    runtime::UseCaseRuntime,
    use_cases::{
        ContactEvent,
        EventHandler,
        TimerId,
        UseCase,
        UseCaseContext,
        UseCaseEvent,
        UseCaseState,
        VicinityEvent,
    },
    utils::ExponentialBackoff,
};

/// Overlay Discovery Configuration.
///
/// Simple data struct.
#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub struct ONDConfig<const BUCKET_SIZE: usize> {
    /// Default minimal [Duration] between subsequent sent FindNodeReq.
    pub send_timeout: Duration,
    /// Minimum [Duration] between subsequent FindNodeReqs for random exploration.
    ///
    /// The interval is doubled after every exploration round that didn't change the
    /// routing table, up to [Self::random_exploration_max_interval], and falls back to
    /// this minimum as soon as the routing table or the underlay vicinity changes.
    pub random_exploration_interval: Duration,
    /// Maximum [Duration] between subsequent FindNodeReqs for random exploration.
    pub random_exploration_max_interval: Duration,
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
}

impl<const BUCKET_SIZE: usize> Default for ONDConfig<BUCKET_SIZE> {
    fn default() -> Self {
        Self {
            // TODO: Useful timeout duration?
            send_timeout: Duration::from_millis(250),
            random_exploration_interval: Duration::from_millis(400),
            random_exploration_max_interval: Duration::from_secs(30),
            overlay_neighborhood_size: NonZeroU64::new(BUCKET_SIZE.try_into().unwrap()).unwrap(),
            backoff_base: NonZeroU32::new(2).unwrap(),
            backoff_max_retries: NonZeroU32::new(10).unwrap(), // 0,25s*1024=256s = 4min 16s
            backoff_starting_duration: Duration::from_millis(250),
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
    /// Current interval of the random exploration
    exploration_interval: Duration,
    /// The routing table or underlay vicinity changed since the last exploration round
    exploration_progress: bool,
    /// The next exploration round refreshes an empty bucket (alternating with random IDs)
    refresh_bucket: bool,
    /// Index of the bucket to refresh next, if empty (round robin over the empty buckets)
    next_refreshed_bucket: usize,
}

impl<C, const BUCKET_SIZE: usize> Default for OverlayNeighborhoodDiscovery<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self::new(ONDConfig::default())
    }
}

impl<C, const BUCKET_SIZE: usize> OverlayNeighborhoodDiscovery<C, BUCKET_SIZE> {
    /// Create a new [OverlayNeighborhoodDiscovery] from an [ONDConfig].
    pub fn new(config: ONDConfig<BUCKET_SIZE>) -> Self {
        Self {
            _pd: PhantomData,
            state: ONDState::Initialized,
            exploration_interval: config.random_exploration_interval,
            exploration_progress: true,
            refresh_bucket: false,
            next_refreshed_bucket: 0,
            config,
        }
    }
}

impl<C, const BUCKET_SIZE: usize> OverlayNeighborhoodDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE> + Debug,
    C::UnderlayNeighborTable:
        ULNTable + Debug + Deref<Target = HashMap<NodeId, (UnderlayNeighborId, ULNState)>>,
{
    /// Changes to the error state: the use case starts running again when it isn't isolated
    /// anymore. Its timers are removed, so that the periodic exploration timer doesn't keep
    /// firing next to the one registered by the restart.
    fn become_isolated(&mut self, context: &C) {
        if let ONDState::Running { timer_contexts, .. } = &self.state {
            for timer_id in timer_contexts.iter().filter_map(|tc| tc.timer_id) {
                context.runtime().remove_timer(timer_id);
            }
        }
        self.state = ONDState::Isolated;
    }

    /// Sets a new timer accordingly and sends a new FindNodeReq with target of our own ID (this is a join)
    /// if exponential backoff allows it.
    fn send_next_join_request(&mut self, context: &C) -> Result<(), <Self as EventHandler>::Error> {
        // No need for discovery if currently isolated
        if context.uln_table().is_empty() {
            log::warn!(target: "overlay_neighborhood_discovery",
                "No underlay neighbors present; Node is isolated"
            );
            self.become_isolated(context);
            return Ok(());
        }

        let current_timer_context =
            if let ONDState::Running { timer_contexts, .. } = &mut self.state {
                &mut timer_contexts[TimerType::PeriodicJoin as usize]
            } else {
                panic!("Called send_next_join_request in non-running state!");
            };

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
        // arm the next join first, so that a round without a usable contact doesn't stop the
        // periodic join (it also heals partitions)
        log::trace!(
            target: "overlay_neighborhood_discovery",
            "Timer next duration: {next_duration:?}"
        );
        current_timer_context.timer_id = Some(context.runtime().register_timer(next_duration));

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

        let closest_via_contacts = context.routing_table().closest(context.root_id(), 3);

        // Get the path to the closest node of ourselves
        // in case the previous FindNodeReq failed, we try to avoid that contact,
        // unless it is the only one
        let closest_on = closest_via_contacts
            .iter()
            .find(|(_, contact)| Some(*contact.id()) != avoid_contact)
            .or(closest_via_contacts.first())
            .map(|(_, contact)| (*contact).clone());

        // No one found -> overlay-wise isolated, but underlay neighbors are present
        // this should never happen as the underlay neighbor are then the closest overlay neighbors
        if closest_on.is_none() {
            log::warn!(
                target: "overlay_neighborhood_discovery",
                "Underlay neighbors are present, but no usable contacts; overlay probably not connected. RT: {:?}, NT: {:?}",
                *context.routing_table(),
                *context.uln_table()
            );
            // no request is pending
            current_timer_context.last_msg_state = None;
            return Ok(());
        }
        let contact = closest_on.unwrap();
        let contact_via_id = *contact.id();
        let route_to_closest_on =
            SourceRoute::new(*context.root_id(), contact.path().unwrap().clone());

        // Get the interface of the next underlay neighbor to route this request through
        let neighbor = route_to_closest_on.current_hop();
        let interface = context.uln_table().get(neighbor).cloned();
        if interface.is_none() {
            log::error!(
                target: "overlay_neighborhood_discovery",
                "Contact is valid but its path goes through an invalid neighbor. Contact: {contact}, neighbor: {neighbor}"
            );
            // no request is pending
            current_timer_context.last_msg_state = None;
            return Ok(());
        }

        // compose FindNodeRequest to our own ID (Join)
        let request = ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::FindNodeReq,
                *context.root_id(),
                // own ID as target
                // message shouldn't loop back to this node because EXACT=false
                *context.root_id(),
                Some(new_nonce),
                *context.uln_table().state_seq_nr(),
                context.uln_table().size(),
            ),
            data: FindNodeReqData {
                neighborhood: self.config.overlay_neighborhood_size,
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

        current_timer_context.last_msg_state = Some((new_nonce, contact_via_id));

        Ok(())
    }

    /// Target of the next exploration round.
    ///
    /// Every other round refreshes an empty bucket (except the deepest one) like Kademlia's
    /// bucket refresh, going round robin over the empty buckets: a random target inside the
    /// bucket leads to the nodes of its subtree, if there are any. A random target finds such a
    /// node only by chance, e.g., the only node of a subtree of 1/8 of the ID space, which takes
    /// long while the exploration backs off. The other rounds use random targets.
    fn exploration_target(&mut self, context: &C) -> NodeId {
        self.refresh_bucket = !self.refresh_bucket;
        if self.refresh_bucket {
            let rt = context.routing_table();
            let empty = empty_buckets(&*rt);
            if let Some(&bucket_index) = empty
                .iter()
                .find(|&&index| index >= self.next_refreshed_bucket)
                .or(empty.first())
            {
                self.next_refreshed_bucket = bucket_index + 1;
                if let Some(target) = random_id_in_bucket(&*rt, bucket_index) {
                    log::trace!(target: "overlay_neighborhood_discovery", "Refreshing empty bucket {bucket_index} with target {target}");
                    return target;
                }
            }
        }
        NodeId::random()
    }

    /// Sends a new FindNodeReq with random target to explore the ID space for new and better contacts and routes
    fn send_next_request_to_random_id(
        &mut self,
        context: &C,
    ) -> Result<(), <Self as EventHandler>::Error> {
        // No need for randomized exploration if currently isolated
        if context.uln_table().is_empty() {
            log::warn!(target: "overlay_neighborhood_discovery",
                       "No underlay neighbors present; Node is isolated"
            );
            self.become_isolated(context);
            return Ok(());
        }

        // adapt the interval: keep exploring quickly while the routing table changes,
        // back off while the exploration doesn't find anything new
        self.exploration_interval = if std::mem::take(&mut self.exploration_progress) {
            self.config.random_exploration_interval
        } else {
            (self.exploration_interval * 2).min(self.config.random_exploration_max_interval)
        };
        let exploration_interval = self.exploration_interval;
        let random_id = self.exploration_target(context);
        let ONDState::Running { timer_contexts, .. } = &mut self.state else {
            panic!("Called send_next_request_to_random_id in non-running state!");
        };
        let current_timer_context = &mut timer_contexts[TimerType::RandomExploration as usize];
        // arm the next round first, so that a round without a usable contact doesn't stop it
        current_timer_context.timer_id =
            Some(context.runtime().register_rand_timer(exploration_interval));
        log::trace!(target: "overlay_neighborhood_discovery", "Next random exploration in about {exploration_interval:?}");

        let new_nonce = Nonce::random();

        let closest_contact_path = context
            .routing_table()
            .closest(&random_id, 1)
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

        let route = SourceRoute::new(*context.root_id(), closest_path);
        let message = ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::FindNodeReq,
                *context.root_id(),
                random_id, //random ID
                Some(new_nonce),
                *context.uln_table().state_seq_nr(),
                context.uln_table().size(),
            ),
            data: FindNodeReqData {
                neighborhood: self.config.overlay_neighborhood_size,
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

/// Returns if the routing table change may be a result of, or a reason for, exploration:
/// contacts were added, removed, replaced, or got another path, state, or StateSeqNr.
///
/// A changed StateSeqNr shows a topology change near the contact. Exploring quickly then
/// spreads the new state (and alternative paths) through the network, as nodes not
/// adjacent to a failure only learn about it from the messages of others.
fn is_exploration_progress(event: &ContactEvent) -> bool {
    match event {
        ContactEvent::New(_) | ContactEvent::Removed(_) | ContactEvent::NewBucket(_) => true,
        ContactEvent::Updated { new, old } => {
            new.id() != old.id()
                || new.state() != old.state()
                || new.state_seq_nr() != old.state_seq_nr()
                || match (new.path(), old.path()) {
                    (Some(new), Some(old)) => !new.is_same_path_as(old),
                    (new, old) => new.is_some() != old.is_some(),
                }
        }
        ContactEvent::Proposed(_) | ContactEvent::BucketUpdated(_) => false,
    }
}

impl<C, const BUCKET_SIZE: usize> OverlayNeighborhoodDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE> + Debug,
    C::UnderlayNeighborTable:
        ULNTable + Debug + Deref<Target = HashMap<NodeId, (UnderlayNeighborId, ULNState)>>,
{
    /// Notes a change that makes exploration worthwhile: the next round uses the minimum
    /// interval and a pending round that is due later is brought forward.
    fn exploration_progress(&mut self, context: &C) {
        let ONDState::Running { timer_contexts, .. } = &mut self.state else {
            return;
        };
        self.exploration_progress = true;
        if self.exploration_interval > self.config.random_exploration_interval {
            let timer_context = &mut timer_contexts[TimerType::RandomExploration as usize];
            if let Some(timer_id) = timer_context.timer_id {
                context.runtime().remove_timer(timer_id);
            }
            self.exploration_interval = self.config.random_exploration_interval;
            timer_context.timer_id = Some(
                context
                    .runtime()
                    .register_rand_timer(self.exploration_interval),
            );
            log::trace!(target: "overlay_neighborhood_discovery", "Routing table changed: next random exploration in about {:?}", self.exploration_interval);
        }
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase for OverlayNeighborhoodDiscovery<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    // Isn't used yet, but provides information for the compiler to derive the BUCKET_SIZE from RT
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE> + Debug,
    C::UnderlayNeighborTable:
        ULNTable + Debug + Deref<Target = HashMap<NodeId, (UnderlayNeighborId, ULNState)>>,
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
        // explore quickly until the routing table settles
        self.exploration_interval = self.config.random_exploration_interval;
        self.exploration_progress = true;

        // the first messages should be sent after a small delay because this
        // node needs to learn its underlay vicinity first, before it can send
        // something meaningful to find its overlay neighbors
        let join_timer_id = context.runtime().register_timer(self.config.send_timeout);
        let random_exploration_timer_id = context
            .runtime()
            .register_rand_timer(self.config.random_exploration_interval);
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
        ULNTable + Debug + Deref<Target = HashMap<NodeId, (UnderlayNeighborId, ULNState)>>,
{
    type Context = C;
    type Error = ONDError;
    type Value = ();

    /// As this reacts to Errors, changes to the RoutingTable must occur before delegating
    /// the event to this method.
    #[instrument(
        target = "overlay_neighborhood_discovery",
        "overlay_neighborhood_discovery",
        skip(self, context, event),
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
                ONDState::Running { timer_contexts, .. },
                UseCaseEvent::Message(
                    ProtocolMessage::FindNodeRsp(ReqRspMessage { common_header, .. }),
                    _,
                ),
            ) => {
                let recvd_nonce = common_header.msg_id();
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
                ONDState::Running { timer_contexts, .. },
                UseCaseEvent::Message(
                    ProtocolMessage::Error(ReqRspMessage {
                        common_header,
                        data,
                        ..
                    }),
                    _,
                ),
            ) => {
                let recvd_nonce = common_header.msg_id();
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
            (ONDState::Running { .. }, UseCaseEvent::Contact(contact_event)) => {
                if is_exploration_progress(&contact_event) {
                    self.exploration_progress(context);
                }
            }
            (
                ONDState::Running { .. },
                UseCaseEvent::Vicinity(VicinityEvent::Changed | VicinityEvent::SSNChanged),
            ) => {
                // the underlay vicinity changed: shorter paths may be found
                self.exploration_progress(context);
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

/// Indices of the empty buckets of the routing table, except the deepest one (which contains
/// the root's own part of the ID space).
fn empty_buckets<'a, const BUCKET_SIZE: usize>(
    rt: &impl RoutingTable<'a, BUCKET_SIZE>,
) -> Vec<usize> {
    let deepest = rt.get_bucket_index(rt.root());
    (0..deepest)
        .filter(|&index| rt.bucket_by_index(index).is_empty())
        .collect()
}

/// A random [NodeId] in the bucket with the given index, or None if none was found.
///
/// The IDs of a bucket share the prefix of the root up to the bucket's level; the remaining
/// bits are drawn until the ID falls into the bucket (one of the buckets of the level).
fn random_id_in_bucket<'a, const BUCKET_SIZE: usize>(
    rt: &impl RoutingTable<'a, BUCKET_SIZE>,
    bucket_index: usize,
) -> Option<NodeId> {
    // the prefix length of the first level is the number of bits per level
    let level_bits = rt.get_bucket_prefix_length(0);
    let shared = rt
        .get_bucket_prefix_length(bucket_index)
        .checked_sub(level_bits)?;
    let root_prefix = rt.root().prefix(shared);
    (0..64)
        .map(|_| {
            let random = NodeId::random();
            root_prefix ^ (random ^ random.prefix(shared))
        })
        .find(|id| rt.get_bucket_index(id) == bucket_index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Output,
        context::{
            ContextConfig,
            SyncContext,
        },
        domain::{
            ConnectionId,
            Contact,
            InterfaceId,
            Path,
            SafeStateSeqNr,
            routing_table::SingleBucketRT,
            underlay_neighbor_table::InMemoryULNTable,
            vicinity::vicinity_graph::PetVicinityGraph,
        },
        runtime::testing::TestingUseCaseRuntime,
    };

    const BUCKET_SIZE: usize = 20;
    type Ctx = SyncContext<
        SingleBucketRT<BUCKET_SIZE>,
        TestingUseCaseRuntime,
        (),
        InMemoryULNTable,
        PetVicinityGraph,
    >;
    type Ond = OverlayNeighborhoodDiscovery<Ctx, BUCKET_SIZE>;

    fn root() -> NodeId {
        NodeId::with_msb(1)
    }
    /// the only underlay neighbor
    fn n() -> NodeId {
        NodeId::with_msb(2)
    }

    /// Leaf node with [n] as underlay neighbor, which isn't a contact yet.
    fn setup() -> (Ond, Ctx) {
        crate::tests::init();
        let ctx = Ctx::new(ContextConfig {
            root_id: root(),
            routing_table: SingleBucketRT::<BUCKET_SIZE>::new(root()),
            runtime: TestingUseCaseRuntime::with_unique_timer_ids(),
            insertion_strategy: (),
            uln_table: InMemoryULNTable::new(),
            vicinity_graph: PetVicinityGraph::new(root()),
        });
        ctx.uln_table_mut().insert(
            n(),
            UnderlayNeighborId {
                interface_id: InterfaceId::try_from(1).unwrap(),
                connection_id: ConnectionId(1),
            },
        );
        let mut ond = Ond::new(ONDConfig::default());
        ond.start(&ctx).unwrap();
        (ond, ctx)
    }

    fn add_contact_n(ctx: &Ctx) {
        let mut path = Path::try_from(&[n()][..]).unwrap();
        path.set_valid();
        ctx.routing_table_mut()
            .add(Contact::new(path, SafeStateSeqNr::try_from(1).unwrap()))
            .unwrap();
    }

    fn join_timer(ond: &Ond) -> Option<TimerId> {
        let ONDState::Running { timer_contexts, .. } = &ond.state else {
            panic!("not running");
        };
        timer_contexts[TimerType::PeriodicJoin as usize].timer_id
    }

    /// Fires the join timer and returns the destinations of the sent joins.
    fn fire_join(ond: &mut Ond, ctx: &Ctx) -> Vec<NodeId> {
        let timer = join_timer(ond).expect("join timer armed");
        ond.handle_event(ctx, UseCaseEvent::Timer(timer)).unwrap();
        ctx.runtime()
            .output()
            .filter_map(|output| match output {
                Output::SendProtocolMessage(ProtocolMessage::FindNodeReq(req), _) => {
                    Some(*req.source_route.current_hop())
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn join_continues_after_round_without_contact() {
        let _clock = crate::tests::TestClock::new();
        let (mut ond, ctx) = setup();

        // n is only a tentative neighbor yet: no contact to send the join to
        let first = join_timer(&ond);
        assert!(fire_join(&mut ond, &ctx).is_empty());
        assert_ne!(join_timer(&ond), first, "next join is armed");

        add_contact_n(&ctx);
        assert_eq!(fire_join(&mut ond, &ctx), vec![n()]);
    }

    #[test]
    fn join_is_retried_via_the_only_contact_without_reply() {
        let _clock = crate::tests::TestClock::new();
        let (mut ond, ctx) = setup();
        add_contact_n(&ctx);

        assert_eq!(fire_join(&mut ond, &ctx), vec![n()]);
        // no reply in time: n is avoided, but it is the only contact
        assert_eq!(fire_join(&mut ond, &ctx), vec![n()]);
        assert_eq!(fire_join(&mut ond, &ctx), vec![n()]);
    }

    fn timer_ids(ond: &Ond) -> Vec<TimerId> {
        let ONDState::Running { timer_contexts, .. } = &ond.state else {
            panic!("not running");
        };
        timer_contexts.iter().filter_map(|tc| tc.timer_id).collect()
    }

    #[test]
    fn isolation_removes_timers_and_restart_registers_new_ones() {
        let _clock = crate::tests::TestClock::new();
        let (mut ond, ctx) = setup();
        let timers = timer_ids(&ond);
        assert_eq!(timers.len(), 2, "join and exploration timer");

        // the only underlay neighbor is lost, the exploration timer notices it
        ctx.uln_table_mut().remove(&n());
        let ONDState::Running { timer_contexts, .. } = &ond.state else {
            unreachable!()
        };
        let exploration = timer_contexts[TimerType::RandomExploration as usize]
            .timer_id
            .unwrap();
        ond.handle_event(&ctx, UseCaseEvent::Timer(exploration))
            .unwrap();
        assert!(matches!(ond.state, ONDState::Isolated));
        let removed = ctx.runtime().removed_timers();
        assert!(timers.iter().all(|timer| removed.contains(timer)));

        // the neighbor is back
        ctx.uln_table_mut().insert(
            n(),
            UnderlayNeighborId {
                interface_id: InterfaceId::try_from(1).unwrap(),
                connection_id: ConnectionId(1),
            },
        );
        ond.handle_event(&ctx, UseCaseEvent::Vicinity(VicinityEvent::Changed))
            .unwrap();
        let restarted = timer_ids(&ond);
        assert_eq!(restarted.len(), 2);
        assert!(restarted.iter().all(|timer| !timers.contains(timer)));
    }

    /// Timer and interval of the random exploration
    fn exploration(ond: &Ond) -> (TimerId, Duration) {
        let ONDState::Running { timer_contexts, .. } = &ond.state else {
            panic!("not running");
        };
        (
            timer_contexts[TimerType::RandomExploration as usize]
                .timer_id
                .expect("exploration timer armed"),
            ond.exploration_interval,
        )
    }

    fn fire_exploration(ond: &mut Ond, ctx: &Ctx) -> Duration {
        let (timer, _) = exploration(ond);
        ond.handle_event(ctx, UseCaseEvent::Timer(timer)).unwrap();
        let (next, interval) = exploration(ond);
        assert_ne!(next, timer, "next exploration is armed");
        interval
    }

    fn contact_event(ond: &mut Ond, ctx: &Ctx, event: ContactEvent) {
        ond.handle_event(ctx, UseCaseEvent::Contact(Box::new(event)))
            .unwrap();
    }

    fn contact_n() -> Contact {
        let mut path = Path::try_from(&[n()][..]).unwrap();
        path.set_valid();
        Contact::new(path, SafeStateSeqNr::try_from(1).unwrap())
    }

    #[test]
    fn exploration_backs_off_without_routing_table_changes() {
        let _clock = crate::tests::TestClock::new();
        let (mut ond, ctx) = setup();
        add_contact_n(&ctx);
        let config = ONDConfig::<BUCKET_SIZE>::default();
        let min = config.random_exploration_interval;

        // the first round after the start uses the minimum interval
        assert_eq!(fire_exploration(&mut ond, &ctx), min);
        assert_eq!(fire_exploration(&mut ond, &ctx), min * 2);
        assert_eq!(fire_exploration(&mut ond, &ctx), min * 4);
        for _ in 0..10 {
            fire_exploration(&mut ond, &ctx);
        }
        assert_eq!(
            exploration(&ond).1,
            config.random_exploration_max_interval,
            "interval is capped"
        );
    }

    #[test]
    fn new_contact_brings_exploration_forward() {
        let _clock = crate::tests::TestClock::new();
        let (mut ond, ctx) = setup();
        for _ in 0..5 {
            fire_exploration(&mut ond, &ctx);
        }
        let (backed_off_timer, backed_off) = exploration(&ond);
        let min = ONDConfig::<BUCKET_SIZE>::default().random_exploration_interval;
        assert!(backed_off > min);

        contact_event(&mut ond, &ctx, ContactEvent::New(contact_n()));

        let (timer, interval) = exploration(&ond);
        assert_eq!(interval, min);
        assert_ne!(timer, backed_off_timer, "exploration is rescheduled");
        assert!(ctx.runtime().removed_timers().contains(&backed_off_timer));
        // the round after the change keeps the minimum interval
        assert_eq!(fire_exploration(&mut ond, &ctx), min);
        assert_eq!(fire_exploration(&mut ond, &ctx), min * 2);
    }

    #[test]
    fn contact_update_without_new_path_or_state_is_no_progress() {
        let _clock = crate::tests::TestClock::new();
        let (mut ond, ctx) = setup();
        for _ in 0..3 {
            fire_exploration(&mut ond, &ctx);
        }
        let before = exploration(&ond);

        let old = contact_n();
        let mut new = old.clone();
        new.set_last_seen_now();
        contact_event(
            &mut ond,
            &ctx,
            ContactEvent::Updated {
                new: Box::new(new),
                old: Box::new(old),
            },
        );
        contact_event(&mut ond, &ctx, ContactEvent::BucketUpdated(0));

        assert_eq!(exploration(&ond), before);
    }

    #[test]
    fn changed_path_ssn_and_vicinity_are_progress() {
        let _clock = crate::tests::TestClock::new();
        let progress_path = {
            let old = contact_n();
            let mut new = old.clone();
            let mut path = Path::try_from(&[n(), NodeId::with_msb(3)][..]).unwrap();
            path.set_valid();
            new.set_path(path);
            ContactEvent::Updated {
                new: Box::new(new),
                old: Box::new(old),
            }
        };
        assert!(is_exploration_progress(&progress_path));
        let progress_ssn = {
            let old = contact_n();
            let mut new = old.clone();
            new.update_state_seq_nr(SafeStateSeqNr::try_from(2).unwrap());
            ContactEvent::Updated {
                new: Box::new(new),
                old: Box::new(old),
            }
        };
        assert!(is_exploration_progress(&progress_ssn));

        let (mut ond, ctx) = setup();
        for _ in 0..3 {
            fire_exploration(&mut ond, &ctx);
        }
        ond.handle_event(&ctx, UseCaseEvent::Vicinity(VicinityEvent::SSNChanged))
            .unwrap();
        assert_eq!(
            exploration(&ond).1,
            ONDConfig::<BUCKET_SIZE>::default().random_exploration_interval
        );
    }

    mod bucket_refresh {
        use super::*;
        use crate::domain::routing_table::flat_routing_table::FlatRoutingTable;

        /// Table of the root 0 with one bit per level, whose buckets 0 (prefix 1) and 1 (prefix
        /// 01) are empty and whose deepest bucket 2 holds the contact 0...01.
        fn table() -> FlatRoutingTable<1, 1> {
            let mut table = FlatRoutingTable::<1, 1>::new(NodeId::ZERO).unwrap();
            table
                .add(Contact::new(
                    Path::from(NodeId::ONE),
                    SafeStateSeqNr::try_from(1).unwrap(),
                ))
                .unwrap();
            table.split_bucket(&NodeId::ONE).unwrap();
            table.split_bucket(&NodeId::ONE).unwrap();
            table
        }

        #[test]
        fn empty_buckets_exclude_the_deepest_one() {
            let _clock = crate::tests::TestClock::new();
            let table = table();
            assert_eq!(table.get_bucket_index(&NodeId::ONE), 2);
            assert_eq!(empty_buckets(&table), vec![0, 1]);
        }

        #[test]
        fn random_ids_fall_into_the_bucket() {
            let _clock = crate::tests::TestClock::new();
            let table = table();
            for bucket_index in [0, 1] {
                for _ in 0..100 {
                    let id = random_id_in_bucket(&table, bucket_index).unwrap();
                    assert_eq!(table.get_bucket_index(&id), bucket_index, "{id}");
                }
            }
            // prefix 01
            let id = random_id_in_bucket(&table, 1).unwrap();
            assert_eq!(id.prefix(2), NodeId::with_msb(0b0100_0000));
        }
    }
}
