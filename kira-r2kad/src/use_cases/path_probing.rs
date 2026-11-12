use std::{
    collections::HashMap,
    marker::PhantomData,
    ops::Deref,
    time::{
        Duration,
        Instant,
    },
};

use tracing::instrument;

use crate::{
    domain::{
        Contact,
        ContactState,
        DEFAULT_BUCKET_SIZE,
        Link,
        NodeId,
        Nonce,
        NotViaState,
        NotViaStateList,
        Path,
        ProtocolMessage,
        ProtocolMessageKind,
        RoutingTable,
        SourceRoute,
        StateSeqNr,
        Timestamp,
        ULNState,
        ULNTable,
        UnderlayNeighborId,
        protocol_message::{
            CommonHeader,
            ErrorData,
            ProbeReqData,
            ProbeRspData,
            ReqRspMessage,
            WireFormatMessage,
        },
    },
    use_cases::{
        ContactEvent,
        EventHandler,
        NeverError,
        TimerId,
        UseCase,
        UseCaseContext,
        UseCaseEvent,
        UseCaseRuntime,
        UseCaseState,
    },
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
    /// Maximum duration a [ProbeReq](crate::domain::ProtocolMessage::ProbeReq) is allowed to take
    /// if the probed path has no measured SRTT.
    ///
    /// Default is **250ms**, which covers the largest round-trip times in the Internet.
    pub request_timeout: Duration,
    /// The timeout of a [ProbeReq](crate::domain::ProtocolMessage::ProbeReq) over a path
    /// with a measured SRTT is this multiple of the SRTT.
    pub srtt_timeout_factor: u32,
    /// Lower bound of the timeout derived from the SRTT, so that small processing delays
    /// don't cause spurious timeouts.
    pub min_request_timeout: Duration,
    /// When scheduled probe requests will start after their initial schedule
    pub scheduled_probe_delay: Duration,
    /// Minimum interval between two probes of a contact that sent a lower
    /// [StateSeqNr] than known, e.g., after a restart.
    pub ssn_check_interval: Duration,
}

impl Default for PathProbingConfig {
    fn default() -> Self {
        Self {
            check_interval: Duration::from_secs(1),
            probe_age: Duration::from_secs(40),
            request_timeout: Duration::from_millis(250),
            srtt_timeout_factor: 2,
            min_request_timeout: Duration::from_millis(50),
            scheduled_probe_delay: Duration::from_millis(250),
            ssn_check_interval: Duration::from_secs(10),
        }
    }
}

#[derive(Debug, Eq, PartialEq, Hash, Clone)]
pub enum PathProbeAction {
    ContactActivePath(NodeId),
    ContactProposedPath(NodeId),
    ProbePath(Path),
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
        requests_in_flight: HashMap<Nonce, (NodeId, Path, Timestamp)>,
        /// Scheduled probe request, mainly for path validation of proposed paths
        scheduled_probe_requests: HashMap<PathProbeAction, Timestamp>,
        /// Timer for sending scheduled probe requests
        scheduled_sending_timer: Option<TimerId>,
        /// Last probe of a contact because of a lower StateSeqNr
        ssn_checks: HashMap<NodeId, Timestamp>,
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
    C::UnderlayNeighborTable:
        ULNTable + Deref<Target = HashMap<NodeId, (UnderlayNeighborId, ULNState)>>,
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

    // sends out a single ProbeReq for the given path
    // this is a one shot message without repetition, its timeout timer handles a lost probe
    fn send_single_probe_req_for_path(&mut self, context: &C, path_to_probe: &Path) {
        let timeout = self.request_timeout(path_to_probe);
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

        let route = SourceRoute::new(*context.root_id(), path_to_probe.clone());
        let message = ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::ProbeReq,
                *context.root_id(),
                *route.destination(),
                Some(nonce),
                *context.uln_table().state_seq_nr(),
                context.uln_table().size(),
            ),
            data: ProbeReqData,
            not_via: None,
            source_route: route,
        };
        context
            .runtime()
            .send_message(message, context.uln_table().deref(), context.root_id());

        let timeout_timer = context.runtime().register_timer(timeout);
        probe_timers.insert(timeout_timer, nonce);
        requests_in_flight.insert(
            nonce,
            (
                *path_to_probe.last(),
                path_to_probe.clone(),
                Timestamp::now(),
            ),
        );

        log::trace!(target: "path_probing", "Sent probe {} to {} for path {:?}", nonce, path_to_probe.last(), path_to_probe);
    }

    /// Returns the timeout of a ProbeReq over the path, derived from its SRTT if measured.
    fn request_timeout(&self, path: &Path) -> Duration {
        match path.srtt() {
            Some(srtt) => (srtt * self.config.srtt_timeout_factor).max(self.config.min_request_timeout),
            None => self.config.request_timeout,
        }
    }

    /// Probes a contact that sent us a message with a lower [StateSeqNr] than known,
    /// because only its answer to our request may lower the known one (e.g., after a restart).
    ///
    /// At most one probe per contact is in flight and the checks are rate limited.
    fn check_lower_state_seq_nr(&mut self, context: &C, message: &ProtocolMessage) {
        let PathProbingState::Running {
            requests_in_flight,
            ssn_checks,
            ..
        } = &self.state
        else {
            return;
        };
        let source = message.source();
        let Some(received) = message.source_state_seq_nr().value() else {
            return;
        };
        let contact = {
            let rt = context.routing_table();
            let Some(contact) = rt.contact(source) else {
                return;
            };
            if received >= *contact.state_seq_nr()
                || contact.path().is_none()
                || contact.is_uln()
            {
                // not lower, or a ULN, whose reset is handled by vicinity discovery
                return;
            }
            contact.clone()
        };
        if requests_in_flight.values().any(|(id, _, _)| id == source)
            || ssn_checks
                .get(source)
                .is_some_and(|last| last.to_age_duration() < self.config.ssn_check_interval)
        {
            return;
        }
        log::debug!(target: "path_probing", "Contact {source} sent lower StateSeqNr {received} (known {}), probing it", contact.state_seq_nr());
        if let PathProbingState::Running { ssn_checks, .. } = &mut self.state {
            ssn_checks.insert(*source, Timestamp::now());
        }
        self.send_probe_req(context, &contact);
    }

    fn send_probe_req(&mut self, context: &C, contact: &Contact) {
        let timeout = self.request_timeout(contact.path().unwrap());
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

        let route = SourceRoute::new(*context.root_id(), contact.path().unwrap().clone());
        let message = ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::ProbeReq,
                *context.root_id(),
                *route.destination(),
                Some(nonce),
                *context.uln_table().state_seq_nr(),
                context.uln_table().size(),
            ),
            data: ProbeReqData,
            not_via: None,
            source_route: route,
        };
        context
            .runtime()
            .send_message(message, context.uln_table().deref(), context.root_id());

        let timeout_timer = context.runtime().register_timer(timeout);
        probe_timers.insert(timeout_timer, nonce);
        requests_in_flight.insert(
            nonce,
            (
                *contact.id(),
                contact.path().unwrap().clone(),
                Timestamp::now(),
            ),
        );

        log::trace!(target: "path_probing", "Sent probe {} to {}", nonce, contact.id());
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

    // ProbeReq was not answered timely
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
            let Some((contacts_id, probed_path, ts_probe_sent)) = contact_ts_tuple else {
                // it may be that the contact has been removed meanwhile
                return;
            };
            // note that the ProbeReq may have resulted from probing a proposed path
            // or a path that is not relevant anymore for the current contact
            let mut lock = context.routing_table_mut();
            match lock.contact_mut(&contacts_id) {
                Some(mut contact) => {
                    // invalidate current path but only if it has not been updated meanwhile
                    if let Some(current_path) = contact.path()
                        && current_path.is_same_path_as(&probed_path)
                    {
                        if let Some(last_validated) = current_path.get_last_validated()
                            && *last_validated < ts_probe_sent
                        {
                            contact.set_invalid(NotViaStateList::default());
                            log::trace!(target: "path_probing", "Invalidated contact due to timeout [path: {:?}]", contact.path());
                        }
                    } else if let Some(proposed_path) = contact.proposed_path()
                        && proposed_path.is_same_path_as(&probed_path)
                        && !proposed_path.is_valid()
                    {
                        // a proposed path that couldn't be validated in time is dropped,
                        // so that the next candidate is proposed and probed
                        contact.clear_proposed_path();
                        log::trace!(target: "path_probing", "Removed proposed path of {} due to timeout [path: {probed_path:?}]", contact.id());
                    }
                }
                None => {
                    log::trace!(target: "path_probing", "Removed timeout for non existent contact {contacts_id}")
                }
            };
        }
    }

    // error message came back
    fn invalidate_contact_for_message(&mut self, context: &C, nonce: Nonce, failed_link: Link) {
        if let PathProbingState::Running {
            requests_in_flight,
            probe_timers,
            ..
        } = &mut self.state
        {
            let contact_ts_tuple = requests_in_flight.remove(&nonce);
            let (contacts_id, _, _) = contact_ts_tuple
                .expect("It's assumed that for every timer entry a request entry exists");
            probe_timers.retain(|_, v| *v != nonce);

            let mut lock = context.routing_table_mut();
            match lock.contact_mut(&contacts_id) {
                Some(mut contact) => {
                    // comparing the probed_path is not necessary because if the failed_link is contained,
                    // any affected path has a problem
                    // (the probe may have been for the active or the proposed path)
                    if let Some(proposed_path) = contact.proposed_path()
                        && proposed_path.contains_link(&failed_link)
                    {
                        // proposed path does not work, so delete it
                        log::trace!(target: "path_probing", "Probing for proposed path {:?} of contact {} failed with link {}. Removing proposed path.", contact.proposed_path(), contact.id(), failed_link);
                        contact.clear_proposed_path();
                    }
                    if let Some(active_path) = contact.path()
                        && active_path.contains_link(&failed_link)
                    {
                        log::trace!(target: "path_probing", "Invalidating contact {} because of segment failure for link {}", contact.id(),failed_link);
                        contact.set_invalid(NotViaStateList::from(NotViaState::new(
                            failed_link,
                            Timestamp::now(),
                        )));
                    }
                }
                None => {
                    log::trace!(target: "path_probing", "Removed timeout for non existent contact {contacts_id}")
                }
            };
        }
    }

    fn send_probe_rsp(&mut self, context: &C, req: ReqRspMessage<ProbeReqData>) {
        let source = *req.source();
        // the echoed NotVia ages have to include the time since the requester sent them
        let request_delay = context
            .routing_table()
            .one_way_delay(&source)
            .unwrap_or_default();
        let not_via = req.not_via.as_ref().map(|not_via| {
            not_via
                .iter()
                .map(|entry| entry.aged(request_delay))
                .collect()
        });
        let message = ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::ProbeRsp,
                *context.root_id(),
                source,
                Some(req.msg_id()),
                *context.uln_table().state_seq_nr(),
                context.uln_table().size(),
            ),
            data: ProbeRspData,
            not_via,
            source_route: SourceRoute::from_reversed(req.source_route),
        };
        context
            .runtime()
            .send_message(message, context.uln_table().deref(), context.root_id());

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

    /// Time since the probe with the given nonce was sent, if it is still tracked.
    fn elapsed_since_sent(&self, nonce: &Nonce) -> Option<Duration> {
        let PathProbingState::Running {
            requests_in_flight, ..
        } = &self.state
        else {
            return None;
        };
        requests_in_flight
            .get(nonce)
            .map(|(_, _, sent)| sent.to_age_duration())
    }

    fn is_tracked_message(&self, nonce: &Nonce) -> bool {
        match &self.state {
            PathProbingState::Running {
                requests_in_flight, ..
            } => requests_in_flight.contains_key(nonce),
            _ => false,
        }
    }

    fn schedule_path_probe_req(&mut self, context: &C, path_probe_action: PathProbeAction) {
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
            scheduled_probe_requests.entry(path_probe_action.clone()).or_insert_with(|| {
                log::trace!(target: "path_probing", "Scheduled probing for {:?}", path_probe_action);
                Timestamp::now()
            });
        }
    }

    // this function removes single shot probe requests from the inflight_requests that are older than config.request_timeout,
    // requests with a timeout timer are removed when it fires
    fn garbage_collect_requests(&mut self, context: &C) {
        if let PathProbingState::Running {
            requests_in_flight,
            probe_timers,
            ..
        } = &mut self.state
        {
            let now = context.runtime().current_time();
            let old_size = requests_in_flight.len();
            requests_in_flight.retain(|nonce, v| {
                probe_timers.values().any(|timed| timed == nonce)
                    || now.saturating_duration_since(v.2.into()) <= self.config.request_timeout
            });
            if requests_in_flight.len() < old_size {
                log::trace!(target: "path_probing", "Garbage collect: removed {} orphaned requests", old_size-requests_in_flight.len());
            }
        }
    }

    fn send_scheduled_probe_requests(&mut self, context: &C) {
        let now = context.runtime().current_time();
        let mut probes_to_send: HashMap<PathProbeAction, Timestamp> = HashMap::new();
        if let PathProbingState::Running {
            scheduled_probe_requests,
            scheduled_sending_timer,
            ..
        } = &mut self.state
        {
            // extract all elapsed events
            probes_to_send = scheduled_probe_requests
                .extract_if(|_k, time| {
                    now.saturating_duration_since(<&mut Timestamp as Into<Instant>>::into(time))
                        > self.config.scheduled_probe_delay / 2
                })
                .collect();
            // if more recent requests remain, restart timer
            if scheduled_probe_requests.is_empty() {
                // no timer needed to start sending waiting request
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
            for path_probe_action in probes_to_send.keys() {
                match path_probe_action {
                    PathProbeAction::ContactActivePath(node_id) => {
                        if let Some(contact) = rt.contact(node_id)
                            && let Some(active_path) = contact.path()
                        {
                            self.send_single_probe_req_for_path(context, active_path);
                        }
                    }
                    PathProbeAction::ContactProposedPath(node_id) => {
                        if let Some(contact) = rt.contact(node_id)
                            && let Some(proposed_path) = contact.proposed_path()
                        {
                            self.send_single_probe_req_for_path(context, proposed_path);
                        }
                    }
                    PathProbeAction::ProbePath(path_to_probe) => {
                        self.send_single_probe_req_for_path(context, path_to_probe);
                    }
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
    C::UnderlayNeighborTable:
        ULNTable + Deref<Target = HashMap<NodeId, (UnderlayNeighborId, ULNState)>>,
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
            ssn_checks: HashMap::new(),
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
    C::UnderlayNeighborTable:
        ULNTable + Deref<Target = HashMap<NodeId, (UnderlayNeighborId, ULNState)>>,
{
    type Context = C;
    type Error = NeverError;
    type Value = ();

    #[instrument(
        target = "path_probing",
        "path_probing",
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
                if self.is_tracked_message(&req.msg_id()) =>
            {
                let ReqRspMessage {
                    common_header,
                    source_route,
                    ..
                } = req;
                let source = *source_route.source();
                let mut rev_source_route = Path::from(source_route);
                let my_node_id = rev_source_route.pop().expect("Path should not be empty");
                assert_eq!(my_node_id, *context.root_id());
                rev_source_route.reverse();

                let nonce = common_header.msg_id();
                // round-trip time of this probe, probes are never retransmitted
                let rtt = self.elapsed_since_sent(&nonce);

                // in case we have a contact we validate it explicitly (currently this is done in forward_messages already)
                let mut rt = context.routing_table_mut();
                if let Some(mut contact) = rt.contact_mut(&source) {
                    if let Some(active_path) = contact.path_mut()
                        && active_path.is_same_path_as(&rev_source_route)
                    {
                        // this probe message confirms the active path
                        assert!(active_path.is_valid());
                        // set_valid() also includes update_last_validated();
                        active_path.set_valid();
                        if let Some(rtt) = rtt {
                            active_path.record_rtt(rtt);
                        }
                        contact.set_last_seen_now();

                        // if same path exists as proposed path, remove it
                        if let Some(proposed_path) = contact.proposed_path()
                            && proposed_path.is_same_path_as(&rev_source_route)
                        {
                            contact.clear_proposed_path();
                        }
                    } else {
                        // either no active path exists or it is different from the ProbeRsp rev_source_route
                        if let Some(proposed_path) = contact.proposed_path_mut()
                            && proposed_path.is_same_path_as(&rev_source_route)
                        {
                            // this probe message confirms the proposed path
                            proposed_path.set_valid();
                            if let Some(rtt) = rtt {
                                proposed_path.record_rtt(rtt);
                            }
                        }
                    }
                    // the answer to our request is authoritative, also for a lower sequence
                    // number (e.g., after a restart of the node), whichever path it took
                    if let StateSeqNr::Value(current_state_seq_nr) = common_header.state_seq_num {
                        if current_state_seq_nr < *contact.state_seq_nr() {
                            log::info!(target: "path_probing", "Contact {source} was reset, accepting its lower StateSeqNr {current_state_seq_nr} (was {})", contact.state_seq_nr());
                        }
                        contact.set_state_seq_nr(current_state_seq_nr);
                    }
                }
                self.remove_from_tracked_messages(nonce);

                log::trace!(target: "path_probing", "Probing destination {source} was successful!");
            }
            UseCaseEvent::Message(ProtocolMessage::Error(req), _)
                if self.is_tracked_message(&req.msg_id()) =>
            {
                if let ErrorData::SegmentFailure { failed_link, .. } = &req.data {
                    self.invalidate_contact_for_message(context, req.msg_id(), failed_link.clone());
                } else {
                    // remove any pending request or timer for the corresponding message
                    if let PathProbingState::Running {
                        requests_in_flight,
                        probe_timers,
                        ..
                    } = &mut self.state
                    {
                        let nonce = req.msg_id();
                        requests_in_flight.remove(&nonce);
                        probe_timers.retain(|_, v| *v != nonce);
                    }
                }
            }
            UseCaseEvent::Message(ref message, _)
                if !matches!(message, ProtocolMessage::ProbeReq(_))
                    && message
                        .source_route()
                        .is_some_and(|route| route.destination() == context.root_id()) =>
            {
                self.check_lower_state_seq_nr(context, message);
            }
            UseCaseEvent::Message(ProtocolMessage::ProbeReq(req), _) => {
                if !req.exact() {
                    tracing::warn!(target: "path_probing", "Received ProbeReq without exact flag");
                }
                let ReqRspMessage {
                    ref common_header,
                    ref source_route,
                    ..
                } = req;
                // in case we also have the source as contact, we touch it
                let source = *source_route.source();
                let mut rev_source_route = Path::from(source_route.clone());
                let my_node_id = rev_source_route.pop().expect("Path should not be empty");
                assert_eq!(my_node_id, *context.root_id());
                rev_source_route.reverse();
                // in case we have a contact we validate it explicitly
                let mut rt = context.routing_table_mut();
                if let Some(mut contact) = rt.contact_mut(&source)
                    && let Some(active_path) = contact.path_mut()
                    && active_path.is_same_path_as(&rev_source_route)
                {
                    // this probe message confirms the active path
                    assert!(active_path.is_valid());
                    // set_valid() also includes update_last_validated();
                    active_path.set_valid();
                    contact.set_last_seen_now();
                    if let StateSeqNr::Value(current_state_seq_nr) = common_header.state_seq_num {
                        contact.update_state_seq_nr(current_state_seq_nr);
                    }
                }
                // sending the response reads the routing table
                drop(rt);
                let request = ProtocolMessage::ProbeReq(req.clone());
                self.send_probe_rsp(context, req);
                self.check_lower_state_seq_nr(context, &request);
            }
            UseCaseEvent::Contact(contact_event) => {
                match *contact_event {
                    ContactEvent::Proposed(proposed_contact) => {
                        // sanity check
                        assert!(
                            context
                                .uln_table()
                                .contains_key(proposed_contact.path().unwrap().first())
                        );
                        log::trace!(target: "path_probing", "Probing for proposed contact with path {}", proposed_contact.path().unwrap());
                        // this is not a contact yet, so we need to use the path
                        self.schedule_path_probe_req(
                            context,
                            PathProbeAction::ProbePath(proposed_contact.path().unwrap().clone()),
                        );
                    }
                    ContactEvent::Updated { new, old } => {
                        // in case there is a new proposed path present, probe it in case it is not a ULN
                        if let Some(proposed_path) = new.proposed_path()
                            && proposed_path.size() > 1
                            && old
                                .proposed_path()
                                .is_none_or(|old_path| !old_path.is_same_path_as(proposed_path))
                        {
                            if new.is_valid() {
                                log::trace!(target: "path_probing", "Probing for contact with newly proposed path {proposed_path}");
                                self.schedule_path_probe_req(
                                    context,
                                    PathProbeAction::ContactProposedPath(*new.id()),
                                );
                            } else if matches!(self.state, PathProbingState::Running { .. }) {
                                // the contact is unusable until a path is validated, e.g., a fast
                                // vicinity alternative of failure handling, so probe right away
                                log::trace!(target: "path_probing", "Probing immediately for invalid contact {} with newly proposed path {proposed_path}", new.id());
                                self.send_single_probe_req_for_path(context, proposed_path);
                            }
                        }
                    }
                    ContactEvent::Removed(contact) => {
                        // remove from scheduled requests then
                        if let PathProbingState::Running {
                            scheduled_probe_requests,
                            ssn_checks,
                            ..
                        } = &mut self.state
                        {
                            ssn_checks.remove(contact.id());
                            scheduled_probe_requests
                                .remove(&PathProbeAction::ContactActivePath(*contact.id()));
                            scheduled_probe_requests
                                .remove(&PathProbeAction::ContactProposedPath(*contact.id()));
                            // inflight requests will either trigger timeout or a response coming back later
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }

        Ok(())
    }
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
            InterfaceId,
            SafeStateSeqNr,
            UnderlayNeighborSource,
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
    type PP = PathProbing<Ctx, BUCKET_SIZE>;

    fn root() -> NodeId {
        NodeId::with_msb(1)
    }
    /// underlay neighbor
    fn a() -> NodeId {
        NodeId::with_msb(2)
    }
    fn b() -> NodeId {
        NodeId::with_msb(3)
    }
    /// probed contact
    fn c() -> NodeId {
        NodeId::with_msb(4)
    }
    fn d() -> NodeId {
        NodeId::with_msb(5)
    }
    fn ssn() -> SafeStateSeqNr {
        SafeStateSeqNr::try_from(1).unwrap()
    }
    fn path(nodes: &[NodeId]) -> Path {
        Path::try_from(nodes).unwrap()
    }

    /// Context with contact [c] via the active path `a, b, d, c`, invalidated if `!valid`.
    fn setup(valid: bool) -> (PP, Ctx) {
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
            a(),
            UnderlayNeighborId {
                interface_id: InterfaceId::try_from(1).unwrap(),
                connection_id: ConnectionId(1),
            },
        );
        let mut contact = Contact::new(path(&[a(), b(), d(), c()]), ssn());
        if !valid {
            contact.set_invalid(NotViaStateList::default());
        }
        ctx.routing_table_mut().add(contact).unwrap();

        let mut pp = PP::new(PathProbingConfig::default());
        pp.start(&ctx).unwrap();
        (pp, ctx)
    }

    /// Proposes the path `a, b, c` for [c] and hands the change to path probing.
    fn propose_shorter_path(pp: &mut PP, ctx: &Ctx) {
        let old = ctx.routing_table().contact(&c()).unwrap().clone();
        let mut proposed = path(&[a(), b(), c()]);
        proposed.set_notvalidated();
        assert!(
            ctx.routing_table_mut()
                .contact_mut(&c())
                .unwrap()
                .assess_path_candidate_and_update(&proposed)
        );
        let new = ctx.routing_table().contact(&c()).unwrap().clone();
        assert!(new.proposed_path().is_some());
        pp.handle_event(
            ctx,
            UseCaseEvent::Contact(Box::new(ContactEvent::Updated {
                new: Box::new(new),
                old: Box::new(old),
            })),
        )
        .unwrap();
    }

    /// ProbeReqs sent since the last call
    fn sent_probe_reqs(ctx: &Ctx) -> Vec<ReqRspMessage<ProbeReqData>> {
        ctx.runtime()
            .output()
            .filter_map(|output| match output {
                Output::SendProtocolMessage(ProtocolMessage::ProbeReq(req), _) => Some(req),
                _ => None,
            })
            .collect()
    }

    fn scheduled(pp: &PP) -> &HashMap<PathProbeAction, Timestamp> {
        let PathProbingState::Running {
            scheduled_probe_requests,
            ..
        } = &pp.state
        else {
            panic!("not running");
        };
        scheduled_probe_requests
    }

    #[test]
    fn proposed_path_of_invalid_contact_is_probed_immediately() {
        let _clock = crate::tests::TestClock::new();
        let (mut pp, ctx) = setup(false);
        propose_shorter_path(&mut pp, &ctx);

        let probes = sent_probe_reqs(&ctx);
        assert_eq!(probes.len(), 1, "one ProbeReq sent right away");
        assert_eq!(
            Path::from(probes[0].source_route.clone()).as_ref(),
            [root(), a(), b(), c()],
            "probe follows the proposed path"
        );
        assert!(scheduled(&pp).is_empty(), "nothing left to schedule");
    }

    #[test]
    fn proposed_path_of_valid_contact_is_scheduled() {
        let _clock = crate::tests::TestClock::new();
        let (mut pp, ctx) = setup(true);
        propose_shorter_path(&mut pp, &ctx);

        assert!(sent_probe_reqs(&ctx).is_empty(), "no immediate probe");
        assert!(scheduled(&pp).contains_key(&PathProbeAction::ContactProposedPath(c())));
    }

    #[test]
    fn failed_probe_removes_proposed_path_also_if_active_path_contains_link() {
        let _clock = crate::tests::TestClock::new();
        let (mut pp, ctx) = setup(false);
        propose_shorter_path(&mut pp, &ctx);
        let probe = sent_probe_reqs(&ctx).pop().unwrap();

        // the link a - b of the proposed and the active path is broken
        let failed_link = Link::new(a(), b());
        let error = ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::Error,
                a(),
                root(),
                Some(probe.common_header.msg_id()),
                StateSeqNr::Value(ssn()),
                1,
            ),
            data: ErrorData::SegmentFailure {
                failed_link: failed_link.clone(),
                source: a(),
            },
            not_via: None,
            source_route: SourceRoute::new(a(), path(&[root()])),
        };
        pp.handle_event(
            &ctx,
            UseCaseEvent::Message(ProtocolMessage::Error(error), UnderlayNeighborSource::Local),
        )
        .unwrap();

        let rt = ctx.routing_table();
        let contact = rt.contact(&c()).unwrap();
        assert!(
            contact.proposed_path().is_none(),
            "broken proposed path removed"
        );
        let ContactState::Invalid(not_via) = contact.state() else {
            panic!("contact stays invalid");
        };
        assert!(not_via.nvs_list.iter().any(|nvs| nvs.link == failed_link));
    }

    /// ProbeRsp of [c] to the given probe, traveled back along the active path.
    fn probe_rsp(probe: &ReqRspMessage<ProbeReqData>) -> ProtocolMessage {
        let mut source_route = SourceRoute::new(c(), path(&[d(), b(), a(), root()]));
        for _ in 0..3 {
            source_route.advance();
        }
        ProtocolMessage::ProbeRsp(ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::ProbeRsp,
                c(),
                root(),
                Some(probe.common_header.msg_id()),
                StateSeqNr::Value(ssn()),
                1,
            ),
            data: ProbeRspData,
            not_via: None,
            source_route,
        })
    }

    #[test]
    fn probe_rsp_measures_rtt_of_active_path() {
        let mut clock = crate::tests::TestClock::new();
        let (mut pp, ctx) = setup(true);
        // active paths are validated in practice
        ctx.routing_table_mut()
            .contact_mut(&c())
            .unwrap()
            .path_mut()
            .unwrap()
            .set_valid();
        let contact = ctx.routing_table().contact(&c()).unwrap().clone();
        assert!(contact.srtt().is_none());
        pp.send_probe_req(&ctx, &contact);
        let probe = sent_probe_reqs(&ctx).pop().unwrap();

        clock.advance(Duration::from_millis(20));
        pp.handle_event(
            &ctx,
            UseCaseEvent::Message(probe_rsp(&probe), UnderlayNeighborSource::Local),
        )
        .unwrap();

        let srtt = ctx.routing_table().contact(&c()).unwrap().srtt();
        assert!(
            srtt.is_some_and(|srtt| srtt >= Duration::from_millis(20)),
            "measured {srtt:?}"
        );
    }

    /// Sends a ProbeReq over the active path of [c] with the given SRTT
    /// and returns the duration of the timeout timer.
    fn probe_timeout_with_srtt(srtt: Option<Duration>) -> Duration {
        let (mut pp, ctx) = setup(true);
        if let Some(srtt) = srtt {
            ctx.routing_table_mut()
                .contact_mut(&c())
                .unwrap()
                .path_mut()
                .unwrap()
                .record_rtt(srtt);
        }
        let contact = ctx.routing_table().contact(&c()).unwrap().clone();
        pp.send_probe_req(&ctx, &contact);
        ctx.runtime().registered_timers().last().unwrap().1
    }

    #[test]
    fn probe_timeout_without_srtt_is_fixed() {
        let _clock = crate::tests::TestClock::new();
        assert_eq!(probe_timeout_with_srtt(None), Duration::from_millis(250));
    }

    #[test]
    fn probe_timeout_is_derived_from_srtt() {
        let _clock = crate::tests::TestClock::new();
        assert_eq!(
            probe_timeout_with_srtt(Some(Duration::from_millis(40))),
            Duration::from_millis(80)
        );
        assert_eq!(
            probe_timeout_with_srtt(Some(Duration::from_millis(400))),
            Duration::from_millis(800),
            "may exceed the fixed timeout"
        );
        assert_eq!(
            probe_timeout_with_srtt(Some(Duration::from_millis(5))),
            Duration::from_millis(50),
            "lower bound"
        );
    }

    #[test]
    fn garbage_collection_keeps_probe_until_its_timeout() {
        let mut clock = crate::tests::TestClock::new();
        let (mut pp, ctx) = setup(true);
        {
            let mut rt = ctx.routing_table_mut();
            let contact = rt.contact_mut(&c()).unwrap();
            let path = contact.path_mut().unwrap();
            // active paths are validated in practice
            path.set_valid();
            path.record_rtt(Duration::from_millis(400));
        }
        clock.advance(Duration::from_millis(1));
        let contact = ctx.routing_table().contact(&c()).unwrap().clone();
        pp.send_probe_req(&ctx, &contact);
        let timer = ctx.runtime().registered_timers().last().unwrap().0;

        // collected after the fixed timeout, but before the derived timeout of 800ms
        clock.advance(Duration::from_millis(500));
        ctx.runtime().set_current_time(clock.now());
        pp.garbage_collect_requests(&ctx);
        clock.advance(Duration::from_millis(300));
        pp.handle_event(&ctx, UseCaseEvent::Timer(timer)).unwrap();

        assert!(
            ctx.routing_table().contact(&c()).unwrap().is_invalid(),
            "timeout invalidates the contact"
        );
    }

    /// The timer and duration of the last registered timeout
    fn last_timer(ctx: &Ctx) -> (TimerId, Duration) {
        *ctx.runtime().registered_timers().last().unwrap()
    }

    #[test]
    fn lost_probe_of_proposed_path_removes_it() {
        let _clock = crate::tests::TestClock::new();
        let (mut pp, ctx) = setup(false);
        propose_shorter_path(&mut pp, &ctx);
        assert_eq!(sent_probe_reqs(&ctx).len(), 1);
        let (timer, timeout) = last_timer(&ctx);
        assert_eq!(timeout, Duration::from_millis(250), "no SRTT of a proposed path");

        // neither a ProbeRsp nor an error came back
        pp.handle_event(&ctx, UseCaseEvent::Timer(timer)).unwrap();

        let contact = ctx.routing_table().contact(&c()).unwrap().clone();
        assert!(contact.proposed_path().is_none(), "next candidate can be proposed");
        assert!(contact.is_invalid(), "active path is not affected");
    }

    #[test]
    fn timeout_keeps_proposed_path_that_was_replaced() {
        let _clock = crate::tests::TestClock::new();
        let (mut pp, ctx) = setup(false);
        propose_shorter_path(&mut pp, &ctx);
        let (timer, _) = last_timer(&ctx);
        // a better candidate replaced the probed one, it is probed on its own
        let mut better = path(&[a(), c()]);
        better.set_notvalidated();
        assert!(
            ctx.routing_table_mut()
                .contact_mut(&c())
                .unwrap()
                .assess_path_candidate_and_update(&better)
        );

        pp.handle_event(&ctx, UseCaseEvent::Timer(timer)).unwrap();

        let contact = ctx.routing_table().contact(&c()).unwrap().clone();
        assert!(contact.proposed_path().unwrap().is_same_path_as(&better));
    }

    /// Contact [c] is known with StateSeqNr 10.
    fn setup_known_ssn() -> (PP, Ctx) {
        let (pp, ctx) = setup(true);
        {
            let mut rt = ctx.routing_table_mut();
            let contact = rt.contact_mut(&c()).unwrap();
            contact.path_mut().unwrap().set_valid();
            contact.set_state_seq_nr(SafeStateSeqNr::try_from(10).unwrap());
        }
        (pp, ctx)
    }

    /// Message of [c] to us with the given StateSeqNr.
    fn message_from_c(kind: ProtocolMessageKind, nonce: Nonce, c_ssn: u32) -> ProtocolMessage {
        let mut source_route = SourceRoute::new(c(), path(&[d(), b(), a(), root()]));
        for _ in 0..3 {
            source_route.advance();
        }
        let common_header = CommonHeader::new(
            kind,
            c(),
            root(),
            Some(nonce),
            StateSeqNr::from(c_ssn),
            1,
        );
        match kind {
            ProtocolMessageKind::ProbeReq => ProtocolMessage::ProbeReq(ReqRspMessage {
                common_header,
                data: ProbeReqData,
                not_via: None,
                source_route,
            }),
            _ => ProtocolMessage::ProbeRsp(ReqRspMessage {
                common_header,
                data: ProbeRspData,
                not_via: None,
                source_route,
            }),
        }
    }

    fn receive(pp: &mut PP, ctx: &Ctx, message: ProtocolMessage) {
        pp.handle_event(ctx, UseCaseEvent::Message(message, UnderlayNeighborSource::Local))
            .unwrap();
    }

    fn known_ssn(ctx: &Ctx) -> SafeStateSeqNr {
        *ctx.routing_table().contact(&c()).unwrap().state_seq_nr()
    }

    #[test]
    fn lower_ssn_of_contact_triggers_one_probe() {
        let _clock = crate::tests::TestClock::new();
        let (mut pp, ctx) = setup_known_ssn();

        // e.g., the contact restarted with the same NodeID
        receive(&mut pp, &ctx, message_from_c(ProtocolMessageKind::ProbeRsp, Nonce::random(), 2));
        receive(&mut pp, &ctx, message_from_c(ProtocolMessageKind::ProbeRsp, Nonce::random(), 3));

        assert_eq!(sent_probe_reqs(&ctx).len(), 1, "one probe in flight");
        assert_eq!(known_ssn(&ctx), SafeStateSeqNr::try_from(10).unwrap(), "not accepted");
    }

    #[test]
    fn higher_ssn_triggers_no_probe() {
        let _clock = crate::tests::TestClock::new();
        let (mut pp, ctx) = setup_known_ssn();

        receive(&mut pp, &ctx, message_from_c(ProtocolMessageKind::ProbeRsp, Nonce::random(), 11));

        assert!(sent_probe_reqs(&ctx).is_empty());
    }

    #[test]
    fn probe_req_with_lower_ssn_is_answered_and_triggers_a_probe() {
        let _clock = crate::tests::TestClock::new();
        let (mut pp, ctx) = setup_known_ssn();

        receive(&mut pp, &ctx, message_from_c(ProtocolMessageKind::ProbeReq, Nonce::random(), 2));

        let sent: Vec<_> = ctx.runtime().output().collect();
        let count = |kind: ProtocolMessageKind| {
            sent.iter()
                .filter(|output| {
                    matches!(output, Output::SendProtocolMessage(message, _) if message.kind() == kind)
                })
                .count()
        };
        assert_eq!(count(ProtocolMessageKind::ProbeRsp), 1, "answered");
        assert_eq!(count(ProtocolMessageKind::ProbeReq), 1, "own probe");
    }

    #[test]
    fn answer_to_probe_lowers_the_known_ssn() {
        let _clock = crate::tests::TestClock::new();
        let (mut pp, ctx) = setup_known_ssn();
        receive(&mut pp, &ctx, message_from_c(ProtocolMessageKind::ProbeRsp, Nonce::random(), 2));
        let probe = sent_probe_reqs(&ctx).pop().unwrap();

        receive(
            &mut pp,
            &ctx,
            message_from_c(ProtocolMessageKind::ProbeRsp, probe.common_header.msg_id(), 2),
        );

        assert_eq!(known_ssn(&ctx), SafeStateSeqNr::try_from(2).unwrap());
    }

    #[test]
    fn untracked_probe_rsp_does_not_measure_rtt() {
        let _clock = crate::tests::TestClock::new();
        let (mut pp, ctx) = setup(true);
        let contact = ctx.routing_table().contact(&c()).unwrap().clone();
        pp.send_probe_req(&ctx, &contact);
        let mut probe = sent_probe_reqs(&ctx).pop().unwrap();
        probe.common_header = CommonHeader::new(
            ProtocolMessageKind::ProbeReq,
            root(),
            c(),
            Some(Nonce::random()),
            StateSeqNr::Value(ssn()),
            1,
        );

        pp.handle_event(
            &ctx,
            UseCaseEvent::Message(probe_rsp(&probe), UnderlayNeighborSource::Local),
        )
        .unwrap();

        assert!(ctx.routing_table().contact(&c()).unwrap().srtt().is_none());
    }

    #[test]
    fn echoed_not_via_in_probe_rsp_includes_request_transit_time() {
        let _clock = crate::tests::TestClock::new();
        let (mut pp, ctx) = setup(true);
        // the underlay neighbor probes us, 100ms one-way delay
        let mut prober = Contact::new(path(&[a()]), ssn());
        let active = prober.path_mut().unwrap();
        active.set_valid();
        active.record_rtt(Duration::from_millis(200));
        ctx.routing_table_mut().add(prober).unwrap();
        let link = Link::new(b(), d());

        let probe = ProtocolMessage::ProbeReq(ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::ProbeReq,
                a(),
                root(),
                Some(Nonce::random()),
                StateSeqNr::Value(ssn()),
                1,
            ),
            data: ProbeReqData,
            not_via: Some(crate::domain::NotViaList::from([
                crate::domain::NotVia::from((link.clone(), crate::domain::Age::from(100))),
            ])),
            source_route: SourceRoute::new(a(), path(&[root()])),
        });
        pp.handle_event(
            &ctx,
            UseCaseEvent::Message(probe, UnderlayNeighborSource::Local),
        )
        .unwrap();

        let rsp = ctx
            .runtime()
            .output()
            .find_map(|output| match output {
                Output::SendProtocolMessage(ProtocolMessage::ProbeRsp(rsp), _) => Some(rsp),
                _ => None,
            })
            .expect("ProbeRsp sent");
        let not_via = rsp.not_via.expect("NotVia echoed");
        assert_eq!(
            not_via.iter().find(|entry| entry.link == link).unwrap().age,
            crate::domain::Age::from(200)
        );
    }
}
