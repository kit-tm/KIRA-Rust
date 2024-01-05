use std::collections::HashSet;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use crate::context::UseCaseContext;
use crate::domain::{
    Contact, ContactState, InsertionStrategy, InsertionStrategyResult, Link, NetworkInterface,
    NodeId, NotVia, Path, RoutingTable,
};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{
    ErrorData, ProtocolMessage, ProtocolMessageSender, RTableData, ReqRspMessage, RouteUpdate,
};
use crate::use_cases::{
    EventHandler, HandlingResult, MessageSentFailed, ReactiveUseCaseState, UseCase, UseCaseEvent,
};

/// Extracts different kinds of information out of incoming [ProtocolMessage]s before
/// forwarding the received message if not addressed to us.
///
/// Extracts these different kinds of information:
///
/// - The [Contact] information of the messages source will be added or updated.
/// - The neighbors [Contact] information as well as
/// - On SegmentFailure: Invalidates all affected contacts
///
/// As some [UseCase]s rely on the information already being extracted this UseCase has to handle
/// any [ProtocolMessage] before all other [UseCase]s.
///
/// The [UseCase] returns an result which shows if the message was already handled and forwarded.
#[derive(Debug)]
pub struct ForwardProtocolMessage<C, const BUCKET_SIZE: usize> {
    _pd: PhantomData<C>,
    state: ReactiveUseCaseState,
}

impl<C, const BUCKET_SIZE: usize> Default for ForwardProtocolMessage<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self {
            _pd: PhantomData::default(),
            state: ReactiveUseCaseState::default(),
        }
    }
}

impl<C, const BUCKET_SIZE: usize> ForwardProtocolMessage<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::InsertionStrategy: InsertionStrategy<C::RoutingTable, BUCKET_SIZE>,
    C::MessageSender: ProtocolMessageSender,
{
    fn extract_path_to_source(&self, message: &ProtocolMessage) -> Path {
        let route = message.source_route().map(SourceRoute::traveled_path);
        let mut path = route.unwrap_or_else(|| Path::from(message.source().clone()));

        path.reverse();

        path
    }

    fn extract_degree_and_neighbor_sum(&self, message: &ProtocolMessage) -> (Option<usize>, Option<NodeId>) {
        match message {
            ProtocolMessage::FindNodeReq(data) => (data.data.origin_degree, data.data.origin_neighbor_id_sum.clone()),
            ProtocolMessage::QueryRouteReq(data) => (data.data.origin_degree, data.data.origin_neighbor_id_sum.clone()),
            _ => (None, None)
        }
    }

    /// Extracts source information, inserts it into the [PNTable] and [RoutingTable] and returns
    /// the extracted [Contact] information.
    fn extract_source_information(
        &self,
        context: &C,
        message: &ProtocolMessage,
        interface: NetworkInterface,
    ) -> Contact {
        let path = self.extract_path_to_source(message);
        let (degree, neighbor_sum) = self.extract_degree_and_neighbor_sum(message);
        let contact = Contact::new(path.clone(), *message.source_state_seq_nr(), degree, neighbor_sum);

        {
            let mut lock = context.pn_table_mut();
            let neighbor_id = path.first();
            if !lock.contains(neighbor_id) {
                if let Some(replaced) = lock.insert(neighbor_id.clone(), interface.clone()) {
                    // Not allowed to happen as lock is held
                    log::warn!(
                        target: "pn_table",
                        "Overwritten interface mapping for '{}' from '{}' to '{}' but checked before",
                        neighbor_id,
                        interface,
                        replaced
                    );
                } else {
                    log::debug!(target: "pn_table", "Inserted neighbor '{}' at interface '{}'", neighbor_id, interface);
                }
            }
        }

        self.update_contact(context, contact.clone());

        contact
    }

    /// Attempts to insert the contact into the routing table which may create a new entry,
    /// update an existing entry or do nothing.
    ///
    /// If the contact was previously a physical neighbor but not anymore its entry in the PNTable
    /// will be removed.
    fn update_contact(&self, context: &C, contact: Contact) {
        if let Some(not_via) = context.not_via().iter().find(|not_via| match not_via {
            NotVia::Link(link) => contact.path().contains_link(link),
        }) {
            log::trace!(target: "forward_protocol_message", "Skipping contact as contains invalid not-via data {}; {}", not_via, contact);
            return;
        }

        log::trace!(target: "forward_protocol_message", "Attempting to insert {}", contact);

        let result = context.routing_table_insertion_strategy().insert(
            contact.clone(),
            context.routing_table_mut().deref_mut(),
            context.pn_table().deref(),
        );

        // Remove if contact changed the routing table in any way, was a physical neighbor and is not a pn anymore
        if result != InsertionStrategyResult::Dropped
            && context.pn_table().contains(contact.id())
            && !context
                .routing_table()
                .contact(contact.id())
                .map(Contact::is_pn)
                .unwrap_or(false)
        {
            context.pn_table_mut().remove(contact.id());
            log::debug!(target: "forward_protocol_message", "Removed {} from PNTable as no more a physical neighbor; {:?}", contact.id(), contact);
        }
    }

    fn extract_rtable_reqrsp(
        &self,
        context: &C,
        request: ReqRspMessage<RTableData>,
        path_to_source: Path,
    ) {
        for mut contact in request.data.contacts {
            let mut path = path_to_source.clone();
            path.extend(contact.path().clone());
            *contact.path_mut() = path;

            self.update_contact(context, contact);
        }
    }

    fn extract_failed_contact(&self, context: &C, request: ReqRspMessage<ErrorData>) {
        match &request.data {
            ErrorData::SegmentFailure {
                failed_link: link, ..
            } => {
                let mut not_via = context.not_via_mut();
                let mut routing_table = context.routing_table_mut();

                not_via.insert(NotVia::Link(link.clone()));

                // I do not know what this does that is not included in the for loop below
                //let contacts_id = request.request_destination();
                //if let Some(mut contact) = routing_table.contact_mut(contacts_id) {
                //    *contact.state_mut() = ContactState::Invalid;
                //}

                for mut contact in routing_table.iter_mut() {
                    if contact.path().contains_link(link) {
                        log::trace!("Invalidate Contact due to failed link: {:?} {:?}", link, *contact);
                        *contact.state_mut() = ContactState::Invalid;
                    }
                }
            }
            ErrorData::DeadEnd => {
                // In case of DeadEnd a path to a contact couldn't be found
                // No invalidation is needed
            }
        };
    }

    // Extracts not_via data and applies them on the routing table
    fn extract_not_via_data(
        &self,
        context: &C,
        source: &NodeId,
        new_not_via_data: &HashSet<NotVia>,
    ) {
        let mut routing_table = context.routing_table_mut();

        for mut contact in routing_table.iter_mut() {
            let not_via_invalidation = new_not_via_data
                .iter()
                .find(|entry| match entry {
                    NotVia::Link(link) => contact.path().contains_link(link),
                })
                .cloned();
            if not_via_invalidation.is_none() {
                continue;
            }
            *contact.state_mut() = ContactState::Invalid;

            log::trace!(target: "forward_protocol_message", "Invalidated contact {} based on not-via data of {}", contact.id(), source);
        }
    }

    fn handle_update_routes<I: IntoIterator<Item = (Contact, RouteUpdate)>>(
        &self,
        context: &C,
        source_id: &NodeId,
        route_updates: I,
    ) {
        let mut routing_table = context.routing_table_mut();
        let source_contact = routing_table.contact(source_id).cloned();
        if source_contact.is_none() {
            return;
        }
        let source_contact = source_contact.unwrap();

        for (updated_contact, update) in route_updates {
            if routing_table.contact(updated_contact.id()).is_none() {
                // Skip updates which are not contained in own routing table
                continue;
            }

            let mut new_path = source_contact.path().clone();
            new_path.extend(updated_contact.path().clone());

            match update {
                // Invalidate contact -> Every Contact affected by that will be handled in
                //                       FailureHandling use case
                RouteUpdate::Removed => {
                    // Checked for existence before
                    let mut saved_contact =
                        routing_table.contact_mut(updated_contact.id()).unwrap();
                    if saved_contact.path().contains(source_id)
                        && saved_contact.is_older_than(&updated_contact)
                    {
                        *saved_contact.state_mut() = ContactState::Invalid;
                        log::trace!(target: "forward_protocol_message", "Invalidated contact {} based on route update data of {} [Removed]", saved_contact.id(), source_id);
                    }
                }
                RouteUpdate::Updated => {
                    // If the saved contact is via the node which updated -> Update Path of contact
                    // Other Paths are updated while operating
                    let mut old_contact = routing_table.contact_mut(updated_contact.id()).unwrap();
                    if old_contact.path().size() > new_path.size()
                        && old_contact.path().contains(source_id)
                        && old_contact.is_older_than(&updated_contact)
                        && updated_contact.state() == &ContactState::Valid
                    {
                        // This is definitely wrong: The conditions above check whether the old path is longer and the information is newer and the new contact is valid. Why should we invalidate here?
                        //*old_contact.state_mut() = ContactState::Invalid;
                        // TODO use shorter path
                        // TODO check what we actually have to do here
                        //log::trace!(target: "forward_protocol_message", "Invalidated contact {} based on route update data of {} [Worsened]", old_contact.id(), source_id);
                    }
                }
            }
        }
    }

    fn extract_message_info(
        &self,
        context: &C,
        message: ProtocolMessage,
        interface: NetworkInterface,
    ) -> Result<(), MessageSentFailed> {
        if let ProtocolMessage::Hello(_) = message {
            return Ok(());
        }

        if let Some(not_via) = message.not_via() {
            self.extract_not_via_data(context, message.source(), not_via);
        }

        let source_contact = self.extract_source_information(context, &message, interface);

        match message {
            ProtocolMessage::PNDiscReq(msg)
            | ProtocolMessage::PNDiscRsp(msg)
            | ProtocolMessage::QueryRouteRsp(msg)
            | ProtocolMessage::FindNodeRsp(msg) => {
                self.extract_rtable_reqrsp(context, msg, source_contact.path().clone())
            }
            ProtocolMessage::Error(error_rsp) => self.extract_failed_contact(context, error_rsp),
            // These are already covered by source info extraction
            // Explicitly listing to yield compile time errors as soon as chnages happen to ProtocolMessage enum
            ProtocolMessage::UpdateRouteReq(req) => {
                self.handle_update_routes(context, req.source_route.source(), req.contact_actions);
            }
            ProtocolMessage::Hello(_)
            | ProtocolMessage::QueryRouteReq(_)
            | ProtocolMessage::FindNodeReq(_)
            | ProtocolMessage::ProbeReq(_)
            | ProtocolMessage::ProbeRsp(_)
            | ProtocolMessage::PathSetupReq(_)
            | ProtocolMessage::PathTeardownReq(_) => {},
            ProtocolMessage::KellyReq(_) | ProtocolMessage::KellyRsp(_) => {
                log::warn!("Forwarding Kelly message");
            }
        }

        Ok(())
    }

    fn handle_next_hop_failed(
        &self,
        context: &C,
        message: ProtocolMessage,
    ) -> Result<(), <Self as EventHandler>::Error> {
        if message.nonce().is_none() {
            // Messages with no nonce don't require a response
            return Ok(());
        }

        assert!(message.source_route().is_some());
        assert!(message.source_route().unwrap().next_hop().is_some());

        let root_id = context.root_id().clone();
        let failed_link = Link::new(
            root_id.clone(),
            message.source_route().unwrap().next_hop().unwrap().clone(),
        );

        let error_message = ReqRspMessage {
            nonce: message.nonce().unwrap().clone(),
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
            data: ErrorData::SegmentFailure {
                failed_link,
                source: root_id,
            },
            not_via: context.not_via().clone(),
            source_route: SourceRoute::from_reversed(message.source_route().unwrap().clone()),
        };
        log::trace!("Sending SegmentFailure: {:?}", error_message);

        if let Err(e) = context.message_sender_mut().send_message(error_message) {
            log::error!("Failed to reply with error message: {}", e);
            return Err(MessageSentFailed);
        }

        Ok(())
    }

    fn handle_forwarding(
        &self,
        context: &C,
        mut message: ProtocolMessage,
    ) -> Result<HandlingResult, MessageSentFailed> {
        if let ProtocolMessage::KellyReq(_) = message {
            log::warn!("Forwarding KeLLy request: forwarding handler");
        }
        if let ProtocolMessage::KellyRsp(_) = message {
            log::warn!("Forwarding KeLLy response: forwarding handler");
        }

        let source_route = message.source_route().cloned();
        if source_route.is_none() {
            return Ok(HandlingResult::NotHandled);
        }
        let source_route = source_route.unwrap();

        // Current hop has to be us
        if source_route.current_hop() != context.root_id() {
            log::warn!(
                target: "forward_protocol_message",
                "Current hop of message {} is not us [{:?}]",
                source_route.current_hop(),
                message
            );
            return Ok(HandlingResult::Handled);
        }

        let next_hop = source_route.next_hop();

        if let ProtocolMessage::KellyReq(_) = message {
            log::warn!("Forwarding KeLLy request: route: {:?}, next hop: {:?}", source_route, next_hop);
        }
        if let ProtocolMessage::KellyRsp(_) = message {
            log::warn!("Forwarding KeLLy response: route: {:?}, next hop: {:?}", source_route, next_hop);
        }

        // Is directed to us -> nothing to forward
        if next_hop.is_none() {
            //log::debug!("Returning not handled");
            return Ok(HandlingResult::NotHandled);
        }
        let next_hop = next_hop.unwrap();

        // From here on the message is assumed to be for us

        // Check if next link is in not_via data
        if context.not_via().contains(&NotVia::Link(Link::new(
            context.root_id().clone(),
            next_hop.clone(),
        ))) {
            log::trace!("Error next hop in not-via: {}", next_hop);
            self.handle_next_hop_failed(context, message)?;
            return Ok(HandlingResult::Handled);
        }

        // Next hop is not a physical neighbor -> Error -> Drop
        let neighbor_interface = context.pn_table().get(next_hop).cloned();
        if neighbor_interface.is_none() {
            log::trace!("Error next hop not a physical neighbor: {}", next_hop);
            self.handle_next_hop_failed(context, message)?;
            return Ok(HandlingResult::Handled);
        }

        // Advance source route and send on interface
        // Checked route before
        if let Some(route) = message.source_route_mut() {
            route.advance();
        }
        log::trace!(target: "forward_protocol_message", "Forwarding message {:?}", message);
        if let Err(e) = context.message_sender_mut().send_message(message) {
            log::error!("Failed to forward message: {}", e);
            return Err(MessageSentFailed);
        }

        Ok(HandlingResult::Handled)
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase for ForwardProtocolMessage<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::InsertionStrategy: InsertionStrategy<C::RoutingTable, BUCKET_SIZE>,
    C::MessageSender: ProtocolMessageSender,
{
    type State = ReactiveUseCaseState;

    fn start(&mut self, _context: &Self::Context) -> Result<(), Self::Error> {
        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

impl<C, const BUCKET_SIZE: usize> EventHandler for ForwardProtocolMessage<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::InsertionStrategy: InsertionStrategy<C::RoutingTable, BUCKET_SIZE>,
    C::MessageSender: ProtocolMessageSender,
{
    type Context = C;
    type Error = MessageSentFailed;
    type Value = HandlingResult;

    fn handle_event(
        &mut self,
        context: &Self::Context,
        event: UseCaseEvent,
    ) -> Result<Self::Value, Self::Error> {
        if let UseCaseEvent::Message(message, interface) = event {
            self.extract_message_info(context, message.clone(), interface)?;

            self.handle_forwarding(context, message)
        } else {
            Ok(HandlingResult::NotHandled)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::num::NonZeroU64;
    use std::ops::Deref;

    use crate::broadcaster::BusBroadcaster;
    use crate::context::{ContextConfig, SyncContext, UseCaseContext};
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::domain::{
        Contact, ContactState, InsertionStrategyResult, Link, NetworkInterface, NodeId, PNTable,
        Path, RoutingTable, StateSeqNr, TestInsertionStrategy,
    };
    use crate::forwarding::in_memory_tables::InMemoryFwdTables;
    use crate::messaging::source_route::SourceRoute;
    use crate::messaging::ProtocolMessage::{
        FindNodeReq, FindNodeRsp, PNDiscReq, PNDiscRsp, QueryRouteRsp,
    };
    use crate::messaging::{
        AsyncProtocolMessageReceiver, ErrorData, FindNodeReqData, InMemoryMessageChannel, Nonce,
        RTableData, ReqRspMessage,
    };
    use crate::runtime::ImmediateRuntime;
    use crate::use_cases::forward_protocol_message::ForwardProtocolMessage;
    use crate::use_cases::{EventHandler, UseCaseEvent};

    #[test]
    fn extract_source_from_pndiscreq() {
        crate::tests::init();

        let root_id = NodeId::with_msb(1);
        let single_bucket_rt = SingleBucketRT::<20>::new(root_id.clone());
        let pn_table = PNTable::new();
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let (message_hub, _receiver) = InMemoryMessageChannel::default().into_parts();
        let broadcaster = BusBroadcaster::new(10);
        let runtime = ImmediateRuntime::new(broadcaster.clone());
        let context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table: single_bucket_rt,
            pn_table,
            insertion_strategy,
            message_sender: message_hub,
            runtime,
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let mut use_case = ForwardProtocolMessage::default();

        let contact_id = NodeId::with_msb(2);
        let interface = NetworkInterface::new("test");
        let event = UseCaseEvent::Message(
            PNDiscReq(ReqRspMessage {
                nonce: Nonce::random(),
                source_state_seq_nr: StateSeqNr::from(1),
                data: RTableData { contacts: vec![] },
                not_via: HashSet::default(),
                source_route: SourceRoute::from(Path::from([contact_id.clone(), root_id])),
            }),
            interface.clone(),
        );
        let handled_result = use_case.handle_event(&context, event);
        assert!(
            handled_result.is_ok(),
            "Handling returned error: {:?}",
            handled_result
        );

        assert_eq!(context.pn_table().get(&contact_id), Some(&interface));
    }

    #[test]
    fn add_source_from_pndiscreq_to_contacts() {
        crate::tests::init();

        let root_id = NodeId::with_msb(1);
        let single_bucket_rt = SingleBucketRT::<20>::new(root_id.clone());
        let pn_table = PNTable::new();
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::default().into_parts();
        let broadcaster = BusBroadcaster::new(10);
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

        let mut use_case = ForwardProtocolMessage::default();

        let neighbor_id = NodeId::with_msb(2);
        let interface = NetworkInterface::new("test");
        let event = UseCaseEvent::Message(
            PNDiscReq(ReqRspMessage {
                nonce: Nonce::random(),
                source_state_seq_nr: StateSeqNr::from(0),
                data: RTableData { contacts: vec![] },
                not_via: HashSet::default(),
                source_route: SourceRoute::from(Path::from([neighbor_id.clone(), root_id])),
            }),
            interface.clone(),
        );
        let handled_result = use_case.handle_event(&context, event);
        assert!(
            handled_result.is_ok(),
            "Handling returned error: {:?}",
            handled_result
        );

        assert_eq!(context.pn_table().get(&neighbor_id), Some(&interface));

        let rt = context.routing_table();
        let saved_contact = rt.contact(&neighbor_id);
        assert!(saved_contact.is_some(), "No contact found");
        let saved_contact = saved_contact.unwrap();
        assert_eq!(saved_contact.path(), &Path::from(neighbor_id));
        assert_eq!(saved_contact.state_seq_nr(), &StateSeqNr::from(0));
        assert_eq!(saved_contact.state(), &ContactState::Valid);
    }

    #[test]
    fn add_source_to_routing_table() {
        crate::tests::init();

        let interface = NetworkInterface::new("test");

        let root_id = NodeId::with_msb(1);
        let source_id = NodeId::with_msb(2);
        let neighbor_id = NodeId::with_msb(42);

        let single_bucket_rt = SingleBucketRT::<20>::new(root_id.clone());
        let mut pn_table = PNTable::new();
        // NOTE: First element in contacts path has to be a neighbor
        pn_table.insert(neighbor_id.clone(), interface.clone());
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::default().into_parts();
        let broadcaster = BusBroadcaster::new(10);
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

        let mut use_case = ForwardProtocolMessage::default();

        let source_route = SourceRoute::from(Path::from([
            source_id.clone(),
            NodeId::with_msb(13),
            NodeId::with_msb(14),
            NodeId::with_msb(15),
            neighbor_id.clone(),
            root_id.clone(),
        ]))
        // One time advanced so the current_hop is the root_id
        .advanced()
        .advanced()
        .advanced()
        .advanced();
        let event = UseCaseEvent::Message(
            FindNodeReq(ReqRspMessage {
                nonce: Nonce::random(),
                source_state_seq_nr: StateSeqNr::from(5),
                data: FindNodeReqData {
                    exact: false,
                    neighborhood: NonZeroU64::new(3).unwrap(),
                    target: NodeId::with_msb(15),
                },
                not_via: HashSet::default(),
                source_route,
            }),
            interface.clone(),
        );
        let handled_result = use_case.handle_event(&context, event);
        assert!(
            handled_result.is_ok(),
            "Handling returned error: {:?}",
            handled_result
        );

        let rt = context.routing_table();
        let saved_contact = rt.contact(&source_id);
        assert!(saved_contact.is_some(), "No contact found");
        let saved_contact = saved_contact.unwrap();
        assert_eq!(
            saved_contact.path(),
            &Path::from([
                neighbor_id,
                NodeId::with_msb(15),
                NodeId::with_msb(14),
                NodeId::with_msb(13),
                source_id
            ])
        );
        assert_eq!(saved_contact.state_seq_nr(), &StateSeqNr::from(5));
        assert_eq!(saved_contact.state(), &ContactState::Valid);
    }

    #[test]
    fn add_new_neighbor_to_pns() {
        crate::tests::init();

        let root_id = NodeId::with_msb(1);
        let source_id = NodeId::with_msb(2);
        let neighbor_id = NodeId::with_msb(3);

        let interface = NetworkInterface::new("test");

        let single_bucket_rt = SingleBucketRT::<20>::new(root_id.clone());
        let pn_table = PNTable::new();
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::default().into_parts();
        let broadcaster = BusBroadcaster::new(10);
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

        let mut use_case = ForwardProtocolMessage::default();

        let source_route = SourceRoute::from(Path::from([
            source_id.clone(),
            neighbor_id.clone(),
            root_id.clone(),
            NodeId::with_msb(13),
            NodeId::with_msb(14),
            NodeId::with_msb(15),
        ]))
        // One time advanced so the current_hop is the root_id
        .advanced();
        let event = UseCaseEvent::Message(
            FindNodeReq(ReqRspMessage {
                nonce: Nonce::random(),
                source_state_seq_nr: StateSeqNr::from(5),
                data: FindNodeReqData {
                    exact: false,
                    neighborhood: NonZeroU64::new(3).unwrap(),
                    target: NodeId::with_msb(15),
                },
                not_via: HashSet::default(),
                source_route,
            }),
            interface.clone(),
        );
        let handled_result = use_case.handle_event(&context, event);
        assert!(
            handled_result.is_ok(),
            "Handling returned error: {:?}",
            handled_result
        );

        assert_eq!(
            context.pn_table().get(&neighbor_id),
            Some(&interface),
            "Neighbor not inserted in PNTable: {:?}",
            context.pn_table().deref()
        );
    }

    #[test]
    fn extract_infos_from_pn_disc_req() {
        crate::tests::init();

        let root_id = NodeId::with_msb(1);
        let neighbor_id = NodeId::with_msb(2);

        let interface = NetworkInterface::new("test");

        let single_bucket_rt = SingleBucketRT::<20>::new(root_id.clone());
        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::default().into_parts();
        let broadcaster = BusBroadcaster::new(10);
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

        let mut use_case = ForwardProtocolMessage::default();

        let neighbors_neighbors = vec![
            Contact::new(Path::from([NodeId::with_msb(15)]), StateSeqNr::from(15)),
            Contact::new(Path::from([NodeId::with_msb(30)]), StateSeqNr::from(30)),
            Contact::new(Path::from([NodeId::with_msb(42)]), StateSeqNr::from(42)),
        ];

        let source_route = SourceRoute::from(Path::from([neighbor_id.clone(), root_id.clone()]));
        let event = UseCaseEvent::Message(
            PNDiscReq(ReqRspMessage {
                nonce: Nonce::random(),
                source_state_seq_nr: StateSeqNr::from(5),
                data: RTableData {
                    contacts: neighbors_neighbors.clone(),
                },
                not_via: HashSet::default(),
                source_route,
            }),
            interface.clone(),
        );
        let handled_result = use_case.handle_event(&context, event);
        assert!(
            handled_result.is_ok(),
            "Handling returned error: {:?}",
            handled_result
        );

        let rt = context.routing_table();
        for contact in &neighbors_neighbors {
            let saved_contact = rt.contact(contact.id());
            assert!(
                saved_contact.is_some(),
                "No contacted extracted with id '{}'",
                contact.id()
            );
            let saved_contact = saved_contact.unwrap();
            assert_eq!(saved_contact.state(), contact.state());
            assert_eq!(saved_contact.last_seen(), contact.last_seen());
            assert_eq!(saved_contact.state_seq_nr(), contact.state_seq_nr());
            assert_eq!(
                saved_contact.path(),
                &Path::from([neighbor_id.clone(), saved_contact.id().clone()])
            );
        }
    }

    #[test]
    fn extract_infos_from_pn_disc_rsp() {
        crate::tests::init();

        let root_id = NodeId::with_msb(1);
        let neighbor_id = NodeId::with_msb(2);

        let interface = NetworkInterface::new("test");

        let single_bucket_rt = SingleBucketRT::<20>::new(root_id.clone());
        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::default().into_parts();
        let broadcaster = BusBroadcaster::new(10);
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

        let mut use_case = ForwardProtocolMessage::default();

        let neighbors_neighbors = vec![
            Contact::new(Path::from([NodeId::with_msb(15)]), StateSeqNr::from(15)),
            Contact::new(Path::from([NodeId::with_msb(30)]), StateSeqNr::from(30)),
            Contact::new(Path::from([NodeId::with_msb(42)]), StateSeqNr::from(42)),
        ];

        let source_route = SourceRoute::from(Path::from([neighbor_id.clone(), root_id.clone()]));
        let event = UseCaseEvent::Message(
            PNDiscRsp(ReqRspMessage {
                nonce: Nonce::random(),
                source_state_seq_nr: StateSeqNr::from(5),
                data: RTableData {
                    contacts: neighbors_neighbors.clone(),
                },
                not_via: HashSet::default(),
                source_route,
            }),
            interface.clone(),
        );
        let handled_result = use_case.handle_event(&context, event);
        assert!(
            handled_result.is_ok(),
            "Handling returned error: {:?}",
            handled_result
        );

        let rt = context.routing_table();
        for contact in &neighbors_neighbors {
            let saved_contact = rt.contact(contact.id());
            assert!(
                saved_contact.is_some(),
                "No contacted extracted with id '{}'",
                contact.id()
            );
            let saved_contact = saved_contact.unwrap();
            assert_eq!(saved_contact.state(), contact.state());
            assert_eq!(saved_contact.last_seen(), contact.last_seen());
            assert_eq!(saved_contact.state_seq_nr(), contact.state_seq_nr());
            assert_eq!(
                saved_contact.path(),
                &Path::from([neighbor_id.clone(), saved_contact.id().clone()])
            );
        }
    }

    #[test]
    fn extract_infos_from_query_route_rsp() {
        crate::tests::init();

        let root_id = NodeId::with_msb(1);
        let source_id = NodeId::with_msb(2);
        let neighbor_id = NodeId::with_msb(15);

        let interface = NetworkInterface::new("test");

        let single_bucket_rt = SingleBucketRT::<20>::new(root_id.clone());
        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::default().into_parts();
        let broadcaster = BusBroadcaster::new(10);
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

        let mut use_case = ForwardProtocolMessage::default();

        let neighbors_neighbors = vec![
            // Insertions strategy doesn't use cycle_remover or shortener. So the shared neighbor
            // will be included as all the others. Therefore removing it to avoid confusion
            //Contact::new(Path::from([neighbor_id.clone()]), StateSeqNr::from(15)),
            Contact::new(Path::from([NodeId::with_msb(30)]), StateSeqNr::from(30)),
            Contact::new(Path::from([NodeId::with_msb(42)]), StateSeqNr::from(42)),
        ];

        let source_route = SourceRoute::from(Path::from([
            source_id.clone(),
            neighbor_id.clone(),
            root_id.clone(),
        ]))
        .advanced();
        let event = UseCaseEvent::Message(
            QueryRouteRsp(ReqRspMessage {
                nonce: Nonce::random(),
                source_state_seq_nr: StateSeqNr::from(5),
                data: RTableData {
                    contacts: neighbors_neighbors.clone(),
                },
                not_via: HashSet::default(),
                source_route,
            }),
            interface.clone(),
        );
        let handled_result = use_case.handle_event(&context, event);
        assert!(
            handled_result.is_ok(),
            "Handling returned error: {:?}",
            handled_result
        );

        let rt = context.routing_table();
        for contact in &neighbors_neighbors {
            let saved_contact = rt.contact(contact.id());
            assert!(
                saved_contact.is_some(),
                "No contacted extracted with id '{}'",
                contact.id()
            );
            let saved_contact = saved_contact.unwrap();
            assert_eq!(saved_contact.state(), contact.state());
            assert_eq!(saved_contact.last_seen(), contact.last_seen());
            assert_eq!(saved_contact.state_seq_nr(), contact.state_seq_nr());
            assert_eq!(
                saved_contact.path(),
                &Path::from([neighbor_id.clone(), source_id.clone(), contact.id().clone()])
            );
        }
    }

    #[test]
    fn extract_infos_from_find_node_rsp() {
        crate::tests::init();

        let root_id = NodeId::with_msb(1);
        let source_id = NodeId::with_msb(2);
        let neighbor_id = NodeId::with_msb(15);

        let interface = NetworkInterface::new("test");

        let single_bucket_rt = SingleBucketRT::<20>::new(root_id.clone());
        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());
        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);
        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::default().into_parts();
        let broadcaster = BusBroadcaster::new(10);
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

        let mut use_case = ForwardProtocolMessage::default();

        let neighbors_neighbors = vec![
            // Insertions strategy doesn't use cycle_remover or shortener. So the shared neighbor
            // will be included as all the others. Therefore removing it to avoid confusion
            //Contact::new(Path::from([neighbor_id.clone()]), StateSeqNr::from(15)),
            Contact::new(
                Path::from([
                    NodeId::with_msb(16),
                    NodeId::with_msb(48),
                    NodeId::with_msb(30),
                ]),
                StateSeqNr::from(30),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(16), NodeId::with_msb(48)]),
                StateSeqNr::from(48),
            ),
            Contact::new(
                Path::from([NodeId::with_msb(16), NodeId::with_msb(42)]),
                StateSeqNr::from(42),
            ),
        ];

        let source_route = SourceRoute::from(Path::from([
            source_id.clone(),
            NodeId::with_msb(120),
            NodeId::with_msb(127),
            neighbor_id.clone(),
            root_id.clone(),
        ]))
        .advanced()
        .advanced()
        .advanced();
        let event = UseCaseEvent::Message(
            FindNodeRsp(ReqRspMessage {
                nonce: Nonce::random(),
                source_state_seq_nr: StateSeqNr::from(5),
                data: RTableData {
                    contacts: neighbors_neighbors.clone(),
                },
                not_via: HashSet::default(),
                source_route,
            }),
            interface.clone(),
        );
        let handled_result = use_case.handle_event(&context, event);
        assert!(
            handled_result.is_ok(),
            "Handling returned error: {:?}",
            handled_result
        );

        let rt = context.routing_table();
        for contact in &neighbors_neighbors {
            let saved_contact = rt.contact(contact.id());
            assert!(
                saved_contact.is_some(),
                "No contacted extracted with id '{}'",
                contact.id()
            );
            let saved_contact = saved_contact.unwrap();
            assert_eq!(saved_contact.state(), contact.state());
            assert_eq!(saved_contact.last_seen(), contact.last_seen());
            assert_eq!(saved_contact.state_seq_nr(), contact.state_seq_nr());
            let mut expected_path = Path::from([
                neighbor_id.clone(),
                NodeId::with_msb(127),
                NodeId::with_msb(120),
                source_id.clone(),
            ]);
            expected_path.extend(contact.path().clone());
            assert_eq!(saved_contact.path(), &expected_path);
        }
    }

    #[tokio::test]
    async fn forward_to_us_doesnt_forward() {
        crate::tests::init();

        let root_id = NodeId::random();

        let (broadcaster, broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, mut hub_receiver) = InMemoryMessageChannel::default().into_parts();

        // At least one has contact has to be present and valid
        // Otherwise the use case thinks the node is isolated
        let neighbor_id = NodeId::random();
        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(0));

        let mut routing_table = SingleBucketRT::<1>::new(root_id.clone());
        assert!(routing_table.insert(neighbor.clone()).is_ok());
        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), NetworkInterface::new("test"));

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

        let mut use_case = ForwardProtocolMessage::default();

        let message = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: StateSeqNr::from(2),
            data: FindNodeReqData {
                exact: false,
                neighborhood: NonZeroU64::new(20).unwrap(),
                target: NodeId::random(),
            },
            not_via: HashSet::default(),
            source_route: SourceRoute::from(Path::from([neighbor.id().clone(), root_id.clone()]))
                .advanced(),
        };

        let result = use_case.handle_event(
            &sync_context,
            UseCaseEvent::Message(message.into(), NetworkInterface::new("test")),
        );
        assert!(
            result.is_ok(),
            "Handling a valid message returned an error: {:?}",
            result
        );

        assert!(broadcast_receiver.try_recv().is_err());

        let receive = hub_receiver.try_recv().await;
        assert!(
            receive.is_err() || receive.as_ref().unwrap().is_none(),
            "No messages should ne emitted, but these were found: {:?}",
            receive
        );
    }

    #[tokio::test]
    async fn forwarding_works() {
        crate::tests::init();

        let root_id = NodeId::with_lsb(1);

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, mut hub_receiver) = InMemoryMessageChannel::default().into_parts();

        // At least one has contact has to be present and valid
        // Otherwise the use case thinks the node is isolated
        let neighbor_id = NodeId::with_lsb(2);
        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(0));

        let mut routing_table = SingleBucketRT::<1>::new(root_id.clone());
        assert!(routing_table.insert(neighbor.clone()).is_ok());
        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), NetworkInterface::new("test"));

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

        let mut use_case = ForwardProtocolMessage::default();

        let foreign_id = NodeId::with_lsb(3);
        let sent_request = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: StateSeqNr::from(0),
            data: FindNodeReqData {
                exact: false,
                neighborhood: NonZeroU64::new(20).unwrap(),
                target: NodeId::random(),
            },
            not_via: HashSet::default(),
            source_route: SourceRoute::from(Path::from([
                foreign_id,
                root_id.clone(),
                neighbor_id.clone(),
            ])),
        };

        let result = use_case.handle_event(
            &sync_context,
            UseCaseEvent::Message(sent_request.clone().into(), NetworkInterface::new("test")),
        );
        assert!(
            result.is_ok(),
            "Handling a valid message returned an error: {:?}",
            result
        );

        let sent_message = hub_receiver.try_recv().await;
        assert!(sent_message.is_ok(), "Timed out getting forwarded message");
        let message = sent_message.unwrap();
        assert!(message.is_some(), "Received no message from hub");
        let (message, _) = message.unwrap();
        if let FindNodeReq(req) = message {
            assert_eq!(&req.nonce, &sent_request.nonce);
            assert_eq!(req.source(), sent_request.source());
            assert_eq!(req.destination(), sent_request.destination());
            let mut route = sent_request.source_route.clone();
            route.advance();
            assert_eq!(&req.source_route, &route);
            assert_eq!(&req.data, &sent_request.data);
        }
    }

    #[test]
    fn error_invalidates_failed_contact() {
        crate::tests::init();

        let interface = NetworkInterface::new("test");

        let root_id = NodeId::with_lsb(1);
        let neighbor_id = NodeId::with_lsb(2);
        let source_id = NodeId::with_lsb(3);
        let failed_contact_id = NodeId::with_lsb(4);

        let invalid_contact_ids = [
            NodeId::with_lsb(5),
            NodeId::with_lsb(6),
            NodeId::with_lsb(7),
            NodeId::with_lsb(8),
        ];

        let failed_link = Link::new(source_id.clone(), failed_contact_id.clone());

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(1);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(0));
        let failed_contact = Contact::new(
            Path::from([
                neighbor_id.clone(),
                source_id.clone(),
                failed_contact_id.clone(),
            ]),
            StateSeqNr::from(13),
        );
        let mut invalid_contacts = invalid_contact_ids
            .iter()
            .cloned()
            .map(|id| {
                Contact::new(
                    Path::from([
                        neighbor_id.clone(),
                        source_id.clone(),
                        failed_contact_id.clone(),
                        id,
                    ]),
                    StateSeqNr::from(rand::random::<u64>()),
                )
            })
            .collect::<Vec<_>>();

        let mut routing_table = SingleBucketRT::<20>::new(root_id.clone());
        assert!(routing_table.insert(neighbor.clone()).is_ok());
        assert!(routing_table.insert(failed_contact.clone()).is_ok());
        for contact in invalid_contacts.clone() {
            assert!(routing_table.insert(contact).is_ok());
        }
        invalid_contacts.push(failed_contact);

        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());

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

        let mut use_case = ForwardProtocolMessage::default();

        let sent_request = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: StateSeqNr::from(0),
            data: ErrorData::SegmentFailure {
                failed_link,
                source: source_id.clone(),
            },
            not_via: HashSet::default(),
            source_route: SourceRoute::from(Path::from([
                failed_contact_id.clone(),
                source_id.clone(),
                neighbor_id.clone(),
                root_id.clone(),
            ]))
            .advanced()
            .advanced(),
        };

        let result = use_case.handle_event(
            &sync_context,
            UseCaseEvent::Message(sent_request.clone().into(), interface.clone()),
        );
        assert!(
            result.is_ok(),
            "Handling a valid message returned an error: {:?}",
            result
        );

        for contact in invalid_contacts {
            let rt = sync_context.routing_table();
            let invalid_contact = rt.contact(contact.id());
            assert!(
                invalid_contact.is_some(),
                "invalid contact no longer present after error"
            );
            let invalid_contact = invalid_contact.unwrap();
            assert_eq!(
                invalid_contact.state(),
                &ContactState::Invalid,
                "All affected contacts should be invalidated"
            );
        }

        let rt = sync_context.routing_table();
        let valid_contact = rt.contact(&neighbor_id);
        assert!(
            valid_contact.is_some(),
            "invalid contact no longer present after error"
        );
        let valid_contact = valid_contact.unwrap();
        assert_eq!(
            valid_contact.state(),
            &ContactState::Valid,
            "All affected contacts should be invalidated"
        );
    }
}
