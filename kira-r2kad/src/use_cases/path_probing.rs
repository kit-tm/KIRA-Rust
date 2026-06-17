use std::collections::HashMap;
use std::marker::PhantomData;
use std::ops::Deref;
use std::time::{Duration, Instant};
use tracing::{Level, instrument};

use crate::domain::{
    Contact, ContactState, DEFAULT_BUCKET_SIZE, Link, NodeId, NotViaState, NotViaStateList, Path,
    RoutingTable, Timestamp, ULNTable, UnderlayNeighborId,
};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{
    CommonHeader, ErrorData, Nonce, ProbeReqData, ProbeRspData, ProtocolMessage,
    ProtocolMessageKind, ReqRspMessage, WireFormatMessage,
};
use crate::use_cases::{
    ContactEvent, EventHandler, NeverError, TimerId, UseCase, UseCaseContext, UseCaseEvent,
    UseCaseRuntime, UseCaseState,
};

/// Configuration for [PathProbing] [UseCase].
#[derive(Debug)]
pub struct PathProbingConfig {
    /// Interval to perform periodic checks if a [Contact]
    /// is about to expire.
    pub check_interval: Duration,
    /// Maximum age of a [Contact] before it has to be probed.
    ///
    /// Default is **40s** because underlay advertising is about 30s.
    pub probe_age: Duration,
    /// Maximum duration a [ProbeReq](crate::messaging::messages::ProtocolMessage::ProbeReq) is allowed to take.
    pub request_timeout: Duration,
    /// When scheduled probe requests will start after their initial schedule
    pub scheduled_probe_delay: Duration,
}

impl Default for PathProbingConfig {
    fn default() -> Self {
        Self {
            check_interval: Duration::from_secs(1),
            probe_age: Duration::from_secs(40),
            request_timeout: Duration::from_secs(10),
            scheduled_probe_delay: Duration::from_millis(250),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum PathProbingState {
    Initialized,
    Running {
        /// Timer of the periodic task to send probe requests
        probe_timer_id: TimerId,
        /// Maps timeout timers per message.
        probe_timers: HashMap<TimerId, Nonce>,
        /// Maps the in flight requests to their contacts id.
        requests_in_flight: HashMap<Nonce, (NodeId, Instant)>,
        /// Scheduled probe request, mainly for path validation of proposed paths
        scheduled_probe_requests: HashMap<NodeId, Instant>,
        /// Timer for sending scheduled probe requests
        scheduled_sending_timer: Option<TimerId>,
    },
    Error,
}

impl UseCaseState for PathProbingState {
    fn is_error(&self) -> bool {
        &Self::Error == self
    }
}

/// Path probing [UseCase] implementation.
///
/// This periodically scans the whole routing table for obsolete contacts and sends
/// path probing requests to them.
/// If a [ErrorData::SegmentFailure] is
/// returned the
/// [ForwardProtocolMessage](crate::use_cases::forward_protocol_message::ForwardProtocolMessage)
/// [UseCase] will invalidate all affected [Contacts](crate::domain::Contact).
#[derive(Debug)]
pub struct PathProbing<C, const BUCKET_SIZE: usize = DEFAULT_BUCKET_SIZE> {
    _pd: PhantomData<C>,
    state: PathProbingState,
    config: PathProbingConfig,
}

impl<C, const BUCKET_SIZE: usize> PathProbing<C, BUCKET_SIZE> {
    pub fn new(config: PathProbingConfig) -> Self {
        Self {
            _pd: PhantomData,
            state: PathProbingState::Initialized,
            config,
        }
    }
}

impl<C, const BUCKET_SIZE: usize> PathProbing<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    // Send ProbeReqs to two oldest valid contacts per bucket
    fn send_probe_reqs(&mut self, context: &C) {
        let rt = context.routing_table();
        for bucket in rt.bucket_iter() {
            let mut considered_contacts: Vec<_> = bucket
                .into_iter()
                .filter(|contact| {
                    contact.last_seen().to_age_duration() >= self.config.probe_age &&
                        contact.state() == &ContactState::Valid &&
                        // should never be the case
                        !contact.is_uln()
                })
                .collect();

            considered_contacts.sort_unstable_by_key(|contact| contact.last_seen());
            // put oldest to the front
            considered_contacts.reverse();

            // TODO: make configurable
            considered_contacts
                .iter()
                .take(2)
                .for_each(|c| self.send_probe_req(context, c))
        }
    }

    // sends out a single ProbeReq for a proposed path if that is present
    // this is a one shot message, so no timer for repetition is started and garbage collection will remove any unsatisfied request nonces
    fn send_single_probe_req_for_proposed_path(&mut self, context: &C, contact: &Contact) {
        // if there is no proposed path to probe for, just return
        if contact.proposed_path().is_none() {
            return;
        }

        let requests_in_flight = match &mut self.state {
            PathProbingState::Running {
                requests_in_flight, ..
            } => requests_in_flight,
            _ => panic!("Called sending probe outside of running state"),
        };

        // generate new unused nonce for our message
        let mut nonce = Nonce::random();
        while requests_in_flight.contains_key(&nonce) {
            nonce = Nonce::random();
        }

        let mut route = SourceRoute::from(contact.proposed_path().unwrap().clone());
        route.push_front(*context.root_id());
        let message = ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::ProbeReq,
                *context.root_id(),
                *route.destination(),
                Some(nonce.into()),
                Some(From::from(*context.uln_table().state_seq_nr())),
                context.uln_table().size(),
            ),
            data: ProbeReqData,
            not_via: None,
            source_route: route,
        };
        context
            .runtime()
            .send_message(message, context.uln_table().deref());

        requests_in_flight.insert(nonce, (*contact.id(), context.runtime().current_time()));

        log::trace!(target: "path_probing", "Sent probe for proposed path to {} for path {:?}", contact.id(), contact.proposed_path());
    }

    fn send_probe_req(&mut self, context: &C, contact: &Contact) {
        let (probe_timers, requests_in_flight) = match &mut self.state {
            PathProbingState::Running {
                probe_timers,
                requests_in_flight,
                ..
            } => (probe_timers, requests_in_flight),
            _ => panic!("Called sending probe outside of running state"),
        };

        // generate new unused nonce for our message
        let mut nonce = Nonce::random();
        while requests_in_flight.contains_key(&nonce) {
            nonce = Nonce::random();
        }

        let mut route = SourceRoute::from(contact.path().unwrap().clone());
        route.push_front(*context.root_id());
        let message = ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::ProbeReq,
                *context.root_id(),
                *route.destination(),
                Some(nonce.into()),
                Some(From::from(*context.uln_table().state_seq_nr())),
                context.uln_table().size(),
            ),
            data: ProbeReqData,
            not_via: None,
            source_route: route,
        };
        context
            .runtime()
            .send_message(message, context.uln_table().deref());

        let timeout_timer = context
            .runtime()
            .register_timer(self.config.request_timeout);
        probe_timers.insert(timeout_timer, nonce);
        requests_in_flight.insert(nonce, (*contact.id(), context.runtime().current_time()));

        log::trace!(target: "path_probing", "Sent probe to {}", contact.id());
    }

    fn remove_from_tracked_messages(&mut self, nonce: Nonce) {
        if let PathProbingState::Running {
            requests_in_flight,
            probe_timers,
            ..
        } = &mut self.state
        {
            requests_in_flight.remove(&nonce);
            probe_timers.retain(|_, v| v != &nonce);

            log::trace!(target: "path_probing", "Removed message with nonce {nonce:?} from tracked messages");
        }
    }

    fn invalidate_contact_for_timer(&mut self, context: &C, timer_id: TimerId) {
        if let PathProbingState::Running {
            requests_in_flight,
            probe_timers,
            ..
        } = &mut self.state
        {
            let nonce = probe_timers.remove(&timer_id);
            assert!(nonce.is_some(), "called invalidate without timer existence");
            let nonce = nonce.unwrap();
            let contact_ts_tuple = requests_in_flight.remove(&nonce);
            let (contacts_id, _) = contact_ts_tuple
                .expect("It's assumed that for every timer entry a request entry exists");
            let mut lock = context.routing_table_mut();
            match lock.contact_mut(&contacts_id) {
                Some(mut contact) => {
                    contact.set_invalid(NotViaStateList::default());
                    log::warn!(target: "path_probing", "Invalidated contact due of timer [path: {:?}]", contact.path());
                }
                None => {
                    log::warn!(target: "path_probing", "Removed timeout for non existent contact {contacts_id}")
                }
            };
        }
    }

    fn invalidate_contact_for_message(&mut self, context: &C, nonce: Nonce, failed_link: Link) {
        if let PathProbingState::Running {
            requests_in_flight,
            probe_timers,
            ..
        } = &mut self.state
        {
            let contact_ts_tuple = requests_in_flight.remove(&nonce);
            let (contacts_id, _) = contact_ts_tuple
                .expect("It's assumed that for every timer entry a request entry exists");
            probe_timers.retain(|_, v| *v != nonce);

            let mut lock = context.routing_table_mut();
            match lock.contact_mut(&contacts_id) {
                Some(mut contact) => {
                    contact.set_invalid(NotViaStateList::from(NotViaState::new(
                        failed_link,
                        Timestamp::now(),
                    )));
                    log::warn!(target: "path_probing", "Invalidated contact {} because ot segment failure", contact.id())
                }
                None => {
                    log::warn!(target: "path_probing", "Removed timeout for non existent contact {contacts_id}")
                }
            };
        }
    }

    fn send_probe_rsp(&mut self, context: &C, req: ReqRspMessage<ProbeReqData>) {
        let source = *req.source();
        let message = ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::ProbeRsp,
                *context.root_id(),
                source,
                Some(req.msg_id()),
                Some(From::from(*context.uln_table().state_seq_nr())),
                context.uln_table().size(),
            ),
            data: ProbeRspData,
            not_via: req.not_via,
            source_route: SourceRoute::from_reversed(req.source_route),
        };
        context
            .runtime()
            .send_message(message, context.uln_table().deref());

        log::trace!(target: "path_probing", "Sent probe rsp to {source}");
    }

    fn is_periodic_timer(&self, timer_id: &TimerId) -> bool {
        match &self.state {
            PathProbingState::Running { probe_timer_id, .. } => probe_timer_id == timer_id,
            _ => false,
        }
    }

    fn is_tracked_timer(&self, timer_id: &TimerId) -> bool {
        match &self.state {
            PathProbingState::Running { probe_timers, .. } => probe_timers.contains_key(timer_id),
            _ => false,
        }
    }

    fn is_scheduled_probe_timer(&self, timer_id: &TimerId) -> bool {
        match &self.state {
            PathProbingState::Running {
                scheduled_sending_timer,
                ..
            } => {
                matches!(scheduled_sending_timer, Some(scheduled_sending_timer) if scheduled_sending_timer == timer_id)
            }
            _ => false,
        }
    }

    fn is_tracked_message(&self, nonce: &Nonce) -> bool {
        match &self.state {
            PathProbingState::Running {
                requests_in_flight, ..
            } => requests_in_flight.contains_key(nonce),
            _ => false,
        }
    }

    fn schedule_path_probe_req(&mut self, context: &C, contact: &Contact) {
        if let PathProbingState::Running {
            scheduled_probe_requests,
            scheduled_sending_timer,
            ..
        } = &mut self.state
        {
            // start timer for processing if not already done so
            if scheduled_sending_timer.is_none() {
                *scheduled_sending_timer = Some(
                    context
                        .runtime()
                        .register_rand_timer(self.config.scheduled_probe_delay),
                );
            }
            // add request if not yet present
            if !scheduled_probe_requests.contains_key(contact.id()) {
                scheduled_probe_requests.insert(*contact.id(), context.runtime().current_time());
                log::trace!(target: "path_probing", "Scheduled probing for proposed path to {}",contact.id());
            }
        }
    }

    // this function removes single shot probe requests from the inflight_requests that are older than config.request_timeout
    fn garbage_collect_requests(&mut self, context: &C) {
        if let PathProbingState::Running {
            requests_in_flight, ..
        } = &mut self.state
        {
            let now = context.runtime().current_time();
            let old_size = requests_in_flight.len();
            requests_in_flight
                .retain(|_, v| now.saturating_duration_since(v.1) <= self.config.request_timeout);
            if requests_in_flight.len() < old_size {
                log::trace!(target: "path_probing", "Garbage collect: removed {} orphaned requests", old_size-requests_in_flight.len());
            }
        }
    }

    fn send_scheduled_probe_requests(&mut self, context: &C) {
        let now = context.runtime().current_time();
        let mut probes_to_send: HashMap<NodeId, Instant> = HashMap::new();
        if let PathProbingState::Running {
            scheduled_probe_requests,
            scheduled_sending_timer,
            ..
        } = &mut self.state
        {
            // extract all elapsed events
            probes_to_send = scheduled_probe_requests
                .extract_if(|_k, time| {
                    now.saturating_duration_since(*time) > self.config.scheduled_probe_delay / 2
                })
                .collect();
            // if more recent requests remain, restart timer
            if scheduled_probe_requests.is_empty() {
                // no timer needed  to start sending waiting request
                *scheduled_sending_timer = None;
            } else {
                // start a new timer
                *scheduled_sending_timer = Some(
                    context
                        .runtime()
                        .register_rand_timer(self.config.scheduled_probe_delay),
                );
            }
        }

        // if there is something to probe, send the ProbeReq messages
        if !probes_to_send.is_empty() {
            if probes_to_send.len() > 100 {
                log::warn!(target: "path_probing", "{} probes to send: consider rate limiting", probes_to_send.len());
            }
            // send requests (currently not rate limited)
            let rt = context.routing_table();
            for node_id in probes_to_send.keys() {
                if let Some(contact) = rt.contact(node_id) {
                    self.send_single_probe_req_for_proposed_path(context, contact);
                }
            }
        }
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase for PathProbing<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    type State = PathProbingState;

    fn start(&mut self, context: &C) -> Result<(), Self::Error> {
        let timer_id = context
            .runtime()
            .register_periodic_timer(self.config.check_interval);

        self.state = PathProbingState::Running {
            probe_timer_id: timer_id,
            probe_timers: HashMap::new(),
            requests_in_flight: HashMap::new(),
            scheduled_probe_requests: HashMap::new(),
            scheduled_sending_timer: None,
        };

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

impl<C, const BUCKET_SIZE: usize> EventHandler for PathProbing<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    type Context = C;
    type Error = NeverError;
    type Value = ();

    #[instrument(
        level = Level::TRACE,
        target = "path_probing",
        "path_probing",
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
        match event {
            UseCaseEvent::Timer(timer_id) => {
                if self.is_periodic_timer(&timer_id) {
                    self.send_probe_reqs(context);
                    self.garbage_collect_requests(context);
                }
                // A timeout happened before the answer was received => invalidate contact
                if self.is_tracked_timer(&timer_id) {
                    self.invalidate_contact_for_timer(context, timer_id);
                }
                if self.is_scheduled_probe_timer(&timer_id) {
                    self.send_scheduled_probe_requests(context);
                }
            }
            UseCaseEvent::Message(ProtocolMessage::ProbeRsp(req), _)
                if self.is_tracked_message(&req.msg_id().into()) =>
            {
                let ReqRspMessage {
                    common_header,
                    source_route,
                    ..
                } = req;
                let source = *source_route.source();
                let mut rev_source_route = Path::from(source_route);
                rev_source_route.reverse();

                let nonce = Nonce::from(common_header.msg_id());

                // in case we have a contact we validate it explicitly (currently this is done in forward_messages already)
                let mut rt = context.routing_table_mut();
                if let Some(mut contact) = rt.contact_mut(&source)
                    && let Some(active_path) = contact.path_mut()
                    && *active_path == rev_source_route
                {
                    assert!(active_path.is_valid());
                    active_path.set_state(crate::domain::PathState::Valid);
                    active_path.update_last_validated();
                }
                self.remove_from_tracked_messages(nonce);

                log::trace!(target: "path_probing", "Probing destination {source} was successful!");
            }
            UseCaseEvent::Message(ProtocolMessage::Error(req), _)
                if self.is_tracked_message(&req.msg_id().into()) =>
            {
                if let ErrorData::SegmentFailure { failed_link, .. } = &req.data {
                    self.invalidate_contact_for_message(
                        context,
                        req.msg_id().into(),
                        failed_link.clone(),
                    );
                } else {
                    // remove any pending request or timer for the corresponding message
                    if let PathProbingState::Running {
                        requests_in_flight,
                        probe_timers,
                        ..
                    } = &mut self.state
                    {
                        let nonce = req.msg_id().into();
                        requests_in_flight.remove(&nonce);
                        probe_timers.retain(|_, v| *v != nonce);
                    }
                }
            }
            UseCaseEvent::Message(ProtocolMessage::ProbeReq(req), _) => {
                self.send_probe_rsp(context, req);
            }
            UseCaseEvent::Contact(contact_event) => {
                if let ContactEvent::Updated { new, old } = *contact_event {
                    // in case there is a new proposed path present, schedule probing for proposed path in case it is not a ULN
                    if old.proposed_path().is_none()
                        && new.proposed_path().is_some()
                        && new.proposed_path().unwrap().size() > 1
                    {
                        self.schedule_path_probe_req(context, &new);
                    }
                }
            }
            _ => {}
        }

        Ok(())
    }
}
