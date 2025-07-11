use std::collections::HashMap;
use std::marker::PhantomData;
use std::ops::Deref;
use std::time::Duration;
use tracing::{instrument, Level};

use crate::domain::{
    Contact, ContactState, NodeId, RoutingTable, ULNTable, UnderlayNeighborId, DEFAULT_BUCKET_SIZE,
};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{Nonce, ProbeReqData, ProbeRspData, ProtocolMessage, ReqRspMessage};
use crate::use_cases::{
    EventHandler, NeverError, TimerId, UseCase, UseCaseContext, UseCaseEvent, UseCaseRuntime,
    UseCaseState,
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
    pub probe_age: chrono::Duration,
    /// Maximum duration a [ProbeReq](crate::messaging::messages::ProtocolMessage::ProbeReq) is allowed to take.
    pub request_timeout: Duration,
}

impl Default for PathProbingConfig {
    fn default() -> Self {
        Self {
            check_interval: Duration::from_secs(1),
            probe_age: chrono::Duration::seconds(40),
            request_timeout: Duration::from_secs(10),
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
        requests_in_flight: HashMap<Nonce, NodeId>,
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
/// If a [ErrorData::SegmentFailure](crate::messaging::messages::ErrorData::SegmentFailure) is
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

        let mut route = SourceRoute::from(contact.path().clone());
        route.push_front(*context.root_id());
        let message = ReqRspMessage {
            nonce: nonce.clone(),
            source_state_seq_nr: *context.uln_table().state_seq_nr(),
            data: ProbeReqData,
            not_via: context.not_via().clone(),
            source_route: route,
        };
        context
            .runtime()
            .send_message(message, context.uln_table().deref());

        let timeout_timer = context
            .runtime()
            .register_timer(self.config.request_timeout);
        probe_timers.insert(timeout_timer, nonce.clone());
        requests_in_flight.insert(nonce, *contact.id());

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

            log::trace!(target: "path_probing", "Removed message with nonce {:?} from tracked messages", nonce);
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
            let contacts_id = requests_in_flight.remove(&nonce);
            assert!(
                contacts_id.is_some(),
                "Its assumed that for every timer entry a request entry exists"
            );
            let contacts_id = contacts_id.unwrap();
            let mut lock = context.routing_table_mut();
            match lock.contact_mut(&contacts_id) {
                Some(mut contact) => {
                    *contact.state_mut() = ContactState::Invalid;
                    log::warn!(target: "path_probing", "Invalidated contact due of timer [path: {}]", contact.path());
                }
                None => {
                    log::warn!(target: "path_probing", "Removed timeout for non existent contact {}", contacts_id)
                }
            };
        }
    }

    fn invalidate_contact_for_message(&mut self, context: &C, nonce: Nonce) {
        if let PathProbingState::Running {
            requests_in_flight,
            probe_timers,
            ..
        } = &mut self.state
        {
            let contacts_id = requests_in_flight.remove(&nonce);
            assert!(
                contacts_id.is_some(),
                "Its assumed that for every timer entry a request entry exists"
            );
            let contacts_id = contacts_id.unwrap();
            probe_timers.retain(|_, v| *v != nonce);

            let mut lock = context.routing_table_mut();
            match lock.contact_mut(&contacts_id) {
                Some(mut contact) => {
                    *contact.state_mut() = ContactState::Invalid;
                    log::warn!(target: "path_probing", "Invalidated contact {} because ot segment failure", contact.id())
                }
                None => {
                    log::warn!(target: "path_probing", "Removed timeout for non existent contact {}", contacts_id)
                }
            };
        }
    }

    fn send_probe_rsp(&mut self, context: &C, req: ReqRspMessage<ProbeReqData>) {
        let source = *req.source();
        let message = ReqRspMessage {
            nonce: req.nonce,
            source_state_seq_nr: *context.uln_table().state_seq_nr(),
            data: ProbeRspData,
            not_via: context.not_via().clone(),
            source_route: SourceRoute::from_reversed(req.source_route),
        };
        context
            .runtime()
            .send_message(message, context.uln_table().deref());

        log::trace!(target: "path_probing", "Sent probe rsp to {}", source);
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

    fn is_tracked_message(&self, nonce: &Nonce) -> bool {
        match &self.state {
            PathProbingState::Running {
                requests_in_flight, ..
            } => requests_in_flight.contains_key(nonce),
            _ => false,
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
                }
                // A timeout happened before the answer was received => invalidate contact
                if self.is_tracked_timer(&timer_id) {
                    self.invalidate_contact_for_timer(context, timer_id);
                }
            }
            UseCaseEvent::Message(ProtocolMessage::ProbeRsp(req), _) => {
                if self.is_tracked_message(&req.nonce) {
                    let ReqRspMessage {
                        nonce,
                        source_route,
                        ..
                    } = req;
                    let source = *source_route.source();

                    self.remove_from_tracked_messages(nonce);

                    log::trace!(target: "path_probing", "Probing {} was successful!", source);
                }
            }
            UseCaseEvent::Message(ProtocolMessage::Error(req), _) => {
                if self.is_tracked_message(&req.nonce) {
                    self.invalidate_contact_for_message(context, req.nonce);
                }
            }
            UseCaseEvent::Message(ProtocolMessage::ProbeReq(req), _) => {
                self.send_probe_rsp(context, req);
            }
            _ => {}
        }

        Ok(())
    }
}
