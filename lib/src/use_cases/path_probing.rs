use std::collections::HashMap;
use std::marker::PhantomData;
use std::time::Duration;

use crate::context::UseCaseContext;
use crate::domain::{ContactState, NodeId, RoutingTable, DEFAULT_BUCKET_SIZE};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{
    Nonce, ProbeReqData, ProbeRspData, ProtocolMessage, ProtocolMessageSender, ReqRspMessage,
};
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{
    EventHandler, MessageSentFailed, TimerId, UseCase, UseCaseEvent, UseCaseState,
};

/// Configuration for [PathProbing] [UseCase].
#[derive(Debug)]
pub struct PathProbingConfig {
    /// Interval to perform periodic checks if a [Contact] is about to expire.
    pub check_interval: Duration,
    /// Maximum age of a [Contact] before it has to be probed.
    pub probe_age: chrono::Duration,
    /// Maximum duration a [ProbeReq] is allowed to take.
    pub request_timeout: Duration,
}

impl Default for PathProbingConfig {
    fn default() -> Self {
        Self {
            check_interval: Duration::from_secs(1),
            probe_age: chrono::Duration::seconds(2),
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
/// If a [ErrorData::SegmentFailure] is returned the [ForwardProtocolMessage] [UseCase] will
/// invalidate all affected [Contact]s.
pub struct PathProbing<C, const BUCKET_SIZE: usize = DEFAULT_BUCKET_SIZE> {
    _pd: PhantomData<C>,
    state: PathProbingState,
    config: PathProbingConfig,
}

impl<C, const BUCKET_SIZE: usize> PathProbing<C, BUCKET_SIZE> {
    pub fn new(config: PathProbingConfig) -> Self {
        Self {
            _pd: PhantomData::default(),
            state: PathProbingState::Initialized,
            config,
        }
    }
}

impl<C, const BUCKET_SIZE: usize> PathProbing<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::MessageSender: ProtocolMessageSender,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
{
    // Searched for old contacts and sends ProbeReqs to all of them
    fn send_probe_reqs(&mut self, context: &C) -> Result<(), MessageSentFailed> {
        let (probe_timers, requests_in_flight) = match &mut self.state {
            PathProbingState::Running {
                probe_timers,
                requests_in_flight,
                ..
            } => (probe_timers, requests_in_flight),
            _ => panic!("Called sending probe outside of running state"),
        };

        for contact in context.routing_table().iter() {
            if contact.last_seen().to_age_duration() <= self.config.probe_age {
                continue;
            }

            let mut nonce = Nonce::random();
            while requests_in_flight.contains_key(&nonce) {
                nonce = Nonce::random();
            }

            let mut route = SourceRoute::from(contact.path().clone());
            route.push_front(context.root_id().clone());
            let message = ReqRspMessage {
                nonce: nonce.clone(),
                source_state_seq_nr: *context.pn_table().state_seq_nr(),
                data: ProbeReqData,
                source_route: route,
            };
            if let Err(e) = context.message_sender_mut().send(message) {
                log::error!("Failed to send ProbeReq: {}", e);
                return Err(MessageSentFailed);
            }

            let timeout_timer = context
                .runtime()
                .register_timer(self.config.request_timeout);
            probe_timers.insert(timeout_timer, nonce.clone());
            requests_in_flight.insert(nonce, contact.id().clone());
        }

        Ok(())
    }

    fn remove_from_tracked_messages(&mut self, nonce: Nonce) -> Result<(), MessageSentFailed> {
        if let PathProbingState::Running {
            requests_in_flight,
            probe_timers,
            ..
        } = &mut self.state
        {
            requests_in_flight.remove(&nonce);
            probe_timers.retain(|_, v| *v != nonce);
        }

        Ok(())
    }

    fn invalidate_contact_for_timer(
        &mut self,
        context: &C,
        timer_id: TimerId,
    ) -> Result<(), MessageSentFailed> {
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
                Some(mut contact) => *contact.state_mut() = ContactState::Invalid,
                None => {
                    log::warn!("Removed timeout for non existent contact {}", contacts_id)
                }
            };
        }

        Ok(())
    }

    fn invalidate_contact_for_message(
        &mut self,
        context: &C,
        nonce: Nonce,
    ) -> Result<(), MessageSentFailed> {
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
                Some(mut contact) => *contact.state_mut() = ContactState::Invalid,
                None => {
                    log::warn!("Removed timeout for non existent contact {}", contacts_id)
                }
            };
        }

        Ok(())
    }

    fn send_probe_rsp(
        &mut self,
        context: &C,
        req: ReqRspMessage<ProbeReqData>,
    ) -> Result<(), MessageSentFailed> {
        let message = ReqRspMessage {
            nonce: req.nonce,
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
            data: ProbeRspData,
            source_route: SourceRoute::from_reversed(req.source_route),
        };
        if let Err(e) = context.message_sender_mut().send(message) {
            log::error!("Failed to send probe req: {}", e);
            return Err(MessageSentFailed);
        }

        Ok(())
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
    C::MessageSender: ProtocolMessageSender,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
{
    type State = PathProbingState;

    fn start(&mut self, context: &Self::Context) -> Result<(), Self::Error> {
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
    C::MessageSender: ProtocolMessageSender,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
{
    type Context = C;
    type Error = MessageSentFailed;
    type Value = ();

    fn handle_event(
        &mut self,
        context: &Self::Context,
        event: UseCaseEvent,
    ) -> Result<Self::Value, Self::Error> {
        match event {
            UseCaseEvent::Timer(timer_id) => {
                if self.is_periodic_timer(&timer_id) {
                    self.send_probe_reqs(context)?;
                }
                // A timeout happened before the answer was received => invalidate contact
                if self.is_tracked_timer(&timer_id) {
                    self.invalidate_contact_for_timer(context, timer_id)?;
                }
            }
            UseCaseEvent::Message(ProtocolMessage::ProbeRsp(req), _) => {
                if self.is_tracked_message(&req.nonce) {
                    self.remove_from_tracked_messages(req.nonce)?;
                }
            }
            UseCaseEvent::Message(ProtocolMessage::Error(req), _) => {
                if self.is_tracked_message(&req.nonce) {
                    self.invalidate_contact_for_message(context, req.nonce)?;
                }
            }
            UseCaseEvent::Message(ProtocolMessage::ProbeReq(req), _) => {
                self.send_probe_rsp(context, req)?;
            }
            _ => {}
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::{Duration, Utc};

    use crate::context::{SyncContext, UseCaseContext};
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::domain::{
        Contact, ContactState, InsertionStrategyResult, Link, NetworkInterface, NodeId, PNTable,
        Path, RoutingTable, StateSeqNr, TestInsertionStrategy, Timestamp,
    };
    use crate::messaging::source_route::SourceRoute;
    use crate::messaging::tests::ArcSyncInMemoryMessageHub;
    use crate::messaging::{
        ErrorData, Nonce, ProbeReqData, ProbeRspData, ProtocolMessage, ProtocolMessageReceiver,
        ReqRspMessage,
    };
    use crate::runtime::ImmediateRuntime;
    use crate::use_cases::path_probing::PathProbing;
    use crate::use_cases::{EventHandler, UseCase, UseCaseEvent};

    #[test]
    fn path_probe_sent() {
        crate::tests::init();

        let interface = NetworkInterface::new("test");

        let root_id = NodeId::with_lsb(1);
        let neighbor_id = NodeId::with_lsb(2);

        let contact_ids = [
            NodeId::with_lsb(5),
            NodeId::with_lsb(6),
            NodeId::with_lsb(7),
            NodeId::with_lsb(8),
        ];

        let (broadcaster, broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(30);

        let runtime = ImmediateRuntime::new(broadcaster);

        let mut hub = ArcSyncInMemoryMessageHub::new();

        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(0));
        let old_contacts = contact_ids
            .iter()
            .cloned()
            .map(|id| {
                let mut contact = Contact::new(
                    Path::from([neighbor_id.clone(), id]),
                    StateSeqNr::from(rand::random::<u64>()),
                );
                *contact.last_seen_mut() = Timestamp::from(Utc::now() - Duration::days(2));
                contact
            })
            .collect::<Vec<_>>();

        let mut routing_table = SingleBucketRT::<20>::new(root_id.clone());
        assert!(routing_table.insert(neighbor.clone()).is_ok());
        for contact in old_contacts.clone() {
            assert!(routing_table.insert(contact).is_ok());
        }

        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());

        let sync_context = SyncContext::new(
            root_id.clone(),
            routing_table,
            pn_table,
            TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            hub.clone(),
            runtime,
        );

        let mut use_case = PathProbing::new(Default::default());

        let start_result = use_case.start(&sync_context);
        assert!(start_result.is_ok(), "Failed to start use case");

        // Trigger timer
        let latest_timer_id = broadcast_receiver.try_recv();
        assert!(latest_timer_id.is_ok(), "No timer was created");
        let timer_event = latest_timer_id.unwrap();
        let handle_result = use_case.handle_event(&sync_context, timer_event);
        assert!(handle_result.is_ok(), "triggering timer event failed");

        // Check if messages have been created for all contacts
        let mut contacts_probed = HashMap::new();
        loop {
            match hub.try_recv() {
                Ok(Some((ProtocolMessage::ProbeReq(req), _))) => {
                    let route = req.source_route;
                    contacts_probed.insert(route.destination().clone(), route);
                }
                Ok(None) => break,
                answer => panic!("Received invalid answer: {:?}", answer),
            }
        }

        for old_contact in old_contacts {
            let probed_contact = contacts_probed.remove(old_contact.id());
            assert!(
                probed_contact.is_some(),
                "No probe for old contact {} found",
                old_contact.id()
            );
            let probed_route = probed_contact.unwrap();
            assert_eq!(
                &probed_route,
                &SourceRoute::new(root_id.clone(), old_contact.path().clone()),
                "probed pat is invalid"
            );
        }

        assert!(
            contacts_probed.is_empty(),
            "Some contacts were probed that didn't have to bee probed: {:?}",
            contacts_probed
        );
    }

    #[test]
    fn path_probe_answered() {
        crate::tests::init();

        let interface = NetworkInterface::new("test");

        let root_id = NodeId::with_lsb(1);
        let neighbor_id = NodeId::with_lsb(2);
        let contact_id = NodeId::with_lsb(3);

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(30);

        let runtime = ImmediateRuntime::new(broadcaster);

        let mut hub = ArcSyncInMemoryMessageHub::new();

        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(2));
        let contact = Contact::new(Path::from(contact_id.clone()), StateSeqNr::from(3));

        let mut routing_table = SingleBucketRT::<20>::new(root_id.clone());
        assert!(routing_table.insert(neighbor.clone()).is_ok());
        assert!(routing_table.insert(contact.clone()).is_ok());

        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());

        let sync_context = SyncContext::new(
            root_id.clone(),
            routing_table,
            pn_table,
            TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            hub.clone(),
            runtime,
        );

        let mut use_case = PathProbing::new(Default::default());

        let start_result = use_case.start(&sync_context);
        assert!(start_result.is_ok(), "Failed to start use case");

        // Send ProbeReq
        let request = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: StateSeqNr::from(3),
            data: ProbeReqData,
            source_route: SourceRoute::from(Path::from([
                contact_id.clone(),
                neighbor_id.clone(),
                root_id.clone(),
            ]))
            .advanced(),
        };
        let result = use_case.handle_event(
            &sync_context,
            UseCaseEvent::Message(request.clone().into(), interface.clone()),
        );
        assert!(result.is_ok(), "Failed to send message");

        // Expect ProbeRsp
        let sent_message = hub.try_recv();
        assert!(sent_message.is_ok(), "Failed to receive sent message");
        let sent_message = sent_message.unwrap();
        assert!(sent_message.is_some(), "No message was sent");
        let (sent_message, _) = sent_message.unwrap();
        match sent_message {
            ProtocolMessage::ProbeRsp(rsp) => {
                assert_eq!(rsp.nonce, request.nonce, "Nonces should be equal");
                assert_eq!(
                    rsp.source_state_seq_nr,
                    StateSeqNr::from(1),
                    "SSN should be the one of the root"
                );
                assert_eq!(
                    rsp.source_route,
                    SourceRoute::from_reversed(request.source_route),
                    "source route should be reversed"
                );
            }
            message => panic!("Sent unexpected message: {:?}", message),
        }
    }

    #[test]
    fn contact_invalidated_after_timeout() {
        crate::tests::init();

        let interface = NetworkInterface::new("test");

        let root_id = NodeId::with_lsb(1);
        let neighbor_id = NodeId::with_lsb(2);
        let old_contact_not_responding_id = NodeId::with_lsb(3);

        let (broadcaster, broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(30);

        let runtime = ImmediateRuntime::new(broadcaster);

        let hub = ArcSyncInMemoryMessageHub::new();

        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(0));
        let mut old_contact_not_responding = Contact::new(
            Path::from([neighbor_id.clone(), old_contact_not_responding_id.clone()]),
            StateSeqNr::from(3),
        );
        *old_contact_not_responding.last_seen_mut() =
            Timestamp::from(Utc::now() - Duration::days(1));

        let mut routing_table = SingleBucketRT::<20>::new(root_id.clone());
        assert!(routing_table.insert(neighbor.clone()).is_ok());
        assert!(routing_table
            .insert(old_contact_not_responding.clone())
            .is_ok());

        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());

        let sync_context = SyncContext::new(
            root_id.clone(),
            routing_table,
            pn_table,
            TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            hub.clone(),
            runtime.clone(),
        );

        let mut use_case = PathProbing::new(Default::default());

        let start_result = use_case.start(&sync_context);
        assert!(start_result.is_ok(), "Failed to start use case");

        // Trigger periodic timer
        let latest_timer_id = broadcast_receiver.try_recv();
        assert!(latest_timer_id.is_ok(), "No timer was created");
        let timer_event = latest_timer_id.unwrap();
        let handle_result = use_case.handle_event(&sync_context, timer_event.clone());
        assert!(handle_result.is_ok(), "triggering timer event failed");

        // Trigger timer for timeout
        let timeout_timer_id = runtime
            .latest_timer_id()
            .expect("latest timeout should be at least the periodic timer");
        let timeout_timer_event = UseCaseEvent::Timer(timeout_timer_id);
        assert_ne!(
            &timer_event, &timeout_timer_event,
            "Timeout timer for probe has to be created"
        );
        let handle_result = use_case.handle_event(&sync_context, timeout_timer_event.clone());
        assert!(handle_result.is_ok(), "triggering timer event failed");

        // Contact has to be invalidated
        let lock = sync_context.routing_table();
        let contact = lock.contact(&old_contact_not_responding_id);
        assert!(
            contact.is_some(),
            "not responding contact should invalid but was deleted"
        );
        let contact = contact.unwrap();
        assert_eq!(contact.state(), &ContactState::Invalid);
    }

    #[test]
    fn contact_invalidated_after_segment_failure() {
        crate::tests::init();

        let interface = NetworkInterface::new("test");

        let root_id = NodeId::with_lsb(1);
        let neighbor_id = NodeId::with_lsb(2);
        let old_contact_not_responding_id = NodeId::with_lsb(3);

        let (broadcaster, broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(30);

        let runtime = ImmediateRuntime::new(broadcaster);

        let mut hub = ArcSyncInMemoryMessageHub::new();

        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(0));
        let mut old_contact_not_responding = Contact::new(
            Path::from([neighbor_id.clone(), old_contact_not_responding_id.clone()]),
            StateSeqNr::from(3),
        );
        *old_contact_not_responding.last_seen_mut() =
            Timestamp::from(Utc::now() - Duration::days(1));

        let mut routing_table = SingleBucketRT::<20>::new(root_id.clone());
        assert!(routing_table.insert(neighbor.clone()).is_ok());
        assert!(routing_table
            .insert(old_contact_not_responding.clone())
            .is_ok());

        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());

        let sync_context = SyncContext::new(
            root_id.clone(),
            routing_table,
            pn_table,
            TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            hub.clone(),
            runtime.clone(),
        );

        let mut use_case = PathProbing::new(Default::default());

        let start_result = use_case.start(&sync_context);
        assert!(start_result.is_ok(), "Failed to start use case");

        // Trigger periodic timer
        let latest_timer_id = broadcast_receiver.try_recv();
        assert!(latest_timer_id.is_ok(), "No timer was created");
        let timer_event = latest_timer_id.unwrap();
        let handle_result = use_case.handle_event(&sync_context, timer_event.clone());
        assert!(handle_result.is_ok(), "triggering timer event failed");

        let probe_sent = hub.try_recv();
        assert!(probe_sent.is_ok(), "A Probe has to be sent");
        let probe_sent = probe_sent.unwrap();
        assert!(probe_sent.is_some(), "No probe was immediately created");
        let (probe_sent, _) = probe_sent.unwrap();
        assert!(
            matches!(probe_sent, ProtocolMessage::ProbeReq(_)),
            "Sent message has to be a ProbeReq"
        );

        // Trigger error
        let message = ReqRspMessage {
            nonce: probe_sent.nonce().unwrap().clone(),
            source_state_seq_nr: *neighbor.state_seq_nr(),
            data: ErrorData::SegmentFailure(Link::from((
                neighbor_id.clone(),
                old_contact_not_responding_id.clone(),
            ))),
            source_route: SourceRoute::from_reversed(probe_sent.source_route().unwrap().clone()),
        };
        let handle_result = use_case.handle_event(
            &sync_context,
            UseCaseEvent::Message(message.into(), interface.clone()),
        );
        assert!(handle_result.is_ok(), "triggering timer event failed");

        // Contact has to be invalidated
        let lock = sync_context.routing_table();
        let contact = lock.contact(&old_contact_not_responding_id);
        assert!(
            contact.is_some(),
            "not responding contact should invalid but was deleted"
        );
        let contact = contact.unwrap();
        assert_eq!(contact.state(), &ContactState::Invalid);
    }

    #[test]
    fn contact_still_valid_after_successful_response_and_timeout_timer() {
        crate::tests::init();

        let interface = NetworkInterface::new("test");

        let root_id = NodeId::with_lsb(1);
        let neighbor_id = NodeId::with_lsb(2);
        let old_contact_not_responding_id = NodeId::with_lsb(3);

        let (broadcaster, broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(30);

        let runtime = ImmediateRuntime::new(broadcaster);

        let mut hub = ArcSyncInMemoryMessageHub::new();

        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(0));
        let mut old_contact_not_responding = Contact::new(
            Path::from([neighbor_id.clone(), old_contact_not_responding_id.clone()]),
            StateSeqNr::from(3),
        );
        *old_contact_not_responding.last_seen_mut() =
            Timestamp::from(Utc::now() - Duration::days(1));

        let mut routing_table = SingleBucketRT::<20>::new(root_id.clone());
        assert!(routing_table.insert(neighbor.clone()).is_ok());
        assert!(routing_table
            .insert(old_contact_not_responding.clone())
            .is_ok());

        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());

        let sync_context = SyncContext::new(
            root_id.clone(),
            routing_table,
            pn_table,
            TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            hub.clone(),
            runtime.clone(),
        );

        let mut use_case = PathProbing::new(Default::default());

        let start_result = use_case.start(&sync_context);
        assert!(start_result.is_ok(), "Failed to start use case");

        // Trigger periodic timer
        let latest_timer_id = broadcast_receiver.try_recv();
        assert!(latest_timer_id.is_ok(), "No timer was created");
        let timer_event = latest_timer_id.unwrap();
        let handle_result = use_case.handle_event(&sync_context, timer_event.clone());
        assert!(handle_result.is_ok(), "triggering timer event failed");

        let probe_sent = hub.try_recv();
        assert!(probe_sent.is_ok(), "A Probe has to be sent");
        let probe_sent = probe_sent.unwrap();
        assert!(probe_sent.is_some(), "No probe was immediately created");
        let (probe_sent, _) = probe_sent.unwrap();
        assert!(
            matches!(probe_sent, ProtocolMessage::ProbeReq(_)),
            "Sent message has to be a ProbeReq"
        );

        // get timer event now to not fail because of other created timers
        let timeout_timer_id = runtime
            .latest_timer_id()
            .expect("latest timeout should be at least the periodic timer");
        let timeout_timer_event = UseCaseEvent::Timer(timeout_timer_id);
        assert_ne!(
            &timer_event, &timeout_timer_event,
            "Timeout timer for probe has to be created"
        );

        // Trigger successful response
        let message = ReqRspMessage {
            nonce: probe_sent.nonce().unwrap().clone(),
            source_state_seq_nr: *neighbor.state_seq_nr(),
            data: ProbeRspData,
            source_route: SourceRoute::from_reversed(probe_sent.source_route().unwrap().clone()),
        };
        let handle_result = use_case.handle_event(
            &sync_context,
            UseCaseEvent::Message(message.into(), interface.clone()),
        );
        assert!(handle_result.is_ok(), "triggering timer event failed");

        // Trigger timer
        let handle_result = use_case.handle_event(&sync_context, timeout_timer_event.clone());
        assert!(handle_result.is_ok(), "triggering timer event failed");

        // Contact has to be invalidated
        let lock = sync_context.routing_table();
        let contact = lock.contact(&old_contact_not_responding_id);
        assert!(
            contact.is_some(),
            "not responding contact should invalid but was deleted"
        );
        let contact = contact.unwrap();
        assert_eq!(contact.state(), &ContactState::Valid);
    }
}
