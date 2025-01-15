use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use crate::domain::{
    Contact, ContactState, InsertionStrategy, InsertionStrategyResult, Link, NodeId, NotVia, Path,
    RoutingTable, UNTable, UnderlayNeighborId, UnderlayNeighborSource,
};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{ErrorData, ProtocolMessage, RTableData, ReqRspMessage, RouteUpdate};
use crate::use_cases::{
    BroadcastableUseCaseEvent, EventHandler, HandlingResult, NeverError, ReactiveUseCaseState,
    UseCase, UseCaseContext, UseCaseEvent, UseCaseRuntime,
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
/// any [ProtocolMessage] before all other [UseCase]s **except** [ExplicitPathManagement](super::explicit_path_management::ExplicitPathManagement).
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
            _pd: PhantomData,
            state: ReactiveUseCaseState::default(),
        }
    }
}

impl<C, const BUCKET_SIZE: usize> ForwardProtocolMessage<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::InsertionStrategy: InsertionStrategy<C::RoutingTable, C::PhysicalNeighborTable, BUCKET_SIZE>,
    C::PhysicalNeighborTable: UNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    fn extract_path_to_source(&self, message: &ProtocolMessage) -> Path {
        let route = message
            .source_route()
            // one can only trust the so far traversed path to work and exist
            .map(SourceRoute::traveled_path);
        let mut path = match route {
            None => Path::from(*message.source()),
            Some(path) => path,
        };

        path.reverse();

        path
    }

    /// Extracts source information, inserts it into the [PNTable] and [RoutingTable] and returns
    /// the extracted [Contact] information.
    fn extract_source_information(
        &self,
        context: &C,
        message: &ProtocolMessage,
        ulnid: UnderlayNeighborId,
    ) -> Option<Contact> {
        let path = self.extract_path_to_source(message);
        let contact = Contact::new(path.clone(), *message.source_state_seq_nr());

        {
            let mut lock = context.pn_table_mut();
            let neighbor_id = path.first();
            // loopback: the sender was us
            if neighbor_id == context.root_id() {
                return None;
            }

            if !lock.contains(neighbor_id) {
                if let Some(replaced) = lock.insert(*neighbor_id, ulnid) {
                    // Not allowed to happen as lock is held
                    log::warn!(
                        target: "pn_table",
                        "Overwritten ulnid mapping for '{}' from '{}' to '{}' but checked before",
                        neighbor_id,
                        ulnid,
                        replaced
                    );
                } else {
                    log::debug!(target: "pn_table", "Inserted neighbor '{}' at ulnid '{}'", neighbor_id, ulnid);
                }
            }
        }

        self.update_contact(context, contact.clone());

        Some(contact)
    }

    /// Attempts to insert the contact into the routing table which may create a new entry,
    /// update an existing entry or do nothing.
    ///
    /// If the contact was previously a underlay neighbor but not anymore its entry in the PNTable
    /// will be removed.
    fn update_contact(&self, context: &C, contact: Contact) {
        if let Some(not_via) = context.not_via().iter().find(|not_via| match not_via {
            NotVia::Link(link) => contact.path().contains_link(link),
        }) {
            log::trace!(target: "forward_protocol_message", "Skipping contact as contains invalid not-via data {}; {}", not_via, contact);
            return;
        }

        // Inform other UseCases about newer SSN
        // TODO make more efficient by using the insertion strategy to return existing contact
        if let Some(contact_in_rt) = context.routing_table_mut().contact(contact.id()) {
            if contact_in_rt.state_seq_nr() < contact.state_seq_nr() {
                let expected_ssn = *contact.state_seq_nr();
                let event = BroadcastableUseCaseEvent::ResyncNode(*contact.id(), expected_ssn);

                log::trace!(target: "forward_protocol_message", "Inform other UseCases about updated SSN of {}: {:?}", contact.id(), event);
                context.runtime_mut().broadcast_event(event);
            }
        }

        log::trace!(target: "forward_protocol_message", "Attempting to insert {}", contact);

        let result = context.routing_table_insertion_strategy().insert(
            contact.clone(),
            context.routing_table_mut().deref_mut(),
            context.pn_table().deref(),
        );

        // Remove if contact changed the routing table in any way, was a underlay neighbor and is not a pn anymore
        if result != InsertionStrategyResult::Dropped
            && context.pn_table().contains(contact.id())
            && !context
                .routing_table()
                .contact(contact.id())
                .map(Contact::is_pn)
                .unwrap_or(false)
        {
            context.pn_table_mut().remove(contact.id());
            log::trace!(target: "forward_protocol_message", "Removed {} from PNTable as no more a underlay neighbor; {:?}", contact.id(), contact);
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

                let contacts_id = request.request_destination();

                if let Some(mut contact) = routing_table.contact_mut(contacts_id) {
                    *contact.state_mut() = ContactState::Invalid;
                }

                for mut contact in routing_table.iter_mut() {
                    if contact.path().contains_link(link) {
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

            log::debug!(target: "forward_protocol_message", "Invalidated contact {} based on not-via data of {}", contact.id(), source);
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
                    // todo check if comment actually true
                    let mut old_contact = routing_table.contact_mut(updated_contact.id()).unwrap();
                    if old_contact.path().size() > new_path.size()
                        && old_contact.path().contains(source_id)
                        && old_contact.is_older_than(&updated_contact)
                        && updated_contact.state() == &ContactState::Valid
                    {
                        *old_contact.state_mut() = ContactState::Invalid;
                        log::trace!(target: "forward_protocol_message", "Invalidated contact {} based on route update data of {} [Worsened]", old_contact.id(), source_id);
                    }
                }
            }
        }
    }

    fn extract_message_info(
        &self,
        context: &C,
        message: ProtocolMessage,
        ulnid: UnderlayNeighborId,
    ) {
        if let ProtocolMessage::Hello(_) = message {
            return;
        }

        // invalidate contacts based on not-via information
        if let Some(not_via) = message.not_via() {
            self.extract_not_via_data(context, message.source(), not_via);
        }

        let source_contact = self.extract_source_information(context, &message, ulnid);

        match message {
            ProtocolMessage::PNDiscReq(msg)
            | ProtocolMessage::PNDiscRsp(msg)
            | ProtocolMessage::QueryRouteRsp(msg)
            | ProtocolMessage::FindNodeRsp(msg) => {
                if let Some(source_contact) = source_contact {
                    self.extract_rtable_reqrsp(context, msg, source_contact.path().clone())
                }
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
            | ProtocolMessage::PathTeardownReq(_) => {}
            ProtocolMessage::StoreReq(_)
            | ProtocolMessage::StoreRsp(_)
            | ProtocolMessage::FetchReq(_)
            | ProtocolMessage::FetchRsp(_) => {} // todo maybe we need to extract stuff here
        }
    }

    fn handle_next_hop_failed(&self, context: &C, message: ProtocolMessage) {
        if message.nonce().is_none() {
            // Messages with no nonce don't require a response
            return;
        }

        assert!(message.source_route().is_some());
        assert!(message.source_route().unwrap().next_hop().is_some());

        let root_id = *context.root_id();
        let failed_link = Link::new(
            root_id,
            *message.source_route().unwrap().next_hop().unwrap(),
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

        context
            .runtime_mut()
            .send_message(error_message, context.pn_table().deref());
    }

    fn handle_forwarding(&self, context: &C, mut message: ProtocolMessage) -> HandlingResult {
        let source_route = message.source_route().cloned();
        if source_route.is_none() {
            return HandlingResult::NotHandled;
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
            return HandlingResult::Handled;
        }

        let mut next_hop = source_route.next_hop();

        // source route finished
        if next_hop.is_none() {
            let overlay_destination = message.overlay_destination();
            // is directed to us -> nothing to forward
            if overlay_destination.is_none() {
                return HandlingResult::NotHandled;
            }

            log::debug!(
                target: "forward_protocol_message",
                "Searching next overlay hop for message [{:?}]",
                message
            );

            // Overlay Routing
            // todo add unit tests
            // fixme isolated error if only a single node is used
            // fixme remove cycles in SourceRoute on the way back?
            // fixme respect NotVia? [lib/src/use_cases/handle_overlay_discovery.rs:141]
            let overlay_destination = overlay_destination.unwrap();

            // intended overlay destination is us -> nothing to forward
            if overlay_destination == context.root_id() {
                return HandlingResult::NotHandled;
            }

            // todo support other shared_prefix_grouping via config
            let closest_node = context
                .routing_table()
                .next_hop(overlay_destination, 20, 1)
                .expect("Shared Prefix Grouping should be valid");

            // closest known overlay hop is us -> nothing to forward,
            if closest_node.is_none() {
                log::debug!(
                    target: "forward_protocol_message",
                    "Final destination of overlay message is us [{:?}]",
                    message
                );
                return HandlingResult::NotHandled;
            }

            let next_contact = closest_node.unwrap();

            // extend source route to next hop
            if let Some(sr) = message.source_route_mut() {
                sr.extend(next_contact.path().clone())
            }
            next_hop = message.source_route().and_then(SourceRoute::next_hop);

            log::trace!(target: "forward_protocol_message", "Forwarding overlay message to next hop [{:?}]", message);
        }
        let next_hop = next_hop.unwrap();

        // From here on the message is assumed to be for us

        // Check if next link is in not_via data
        if context
            .not_via()
            .contains(&NotVia::Link(Link::new(*context.root_id(), *next_hop)))
        {
            self.handle_next_hop_failed(context, message);
            return HandlingResult::Handled;
        }

        // Next hop is not a underlay neighbor -> Error -> Drop
        let neighbor_ulnid = context.pn_table().get(next_hop).cloned();
        if neighbor_ulnid.is_none() {
            self.handle_next_hop_failed(context, message);
            return HandlingResult::Handled;
        }

        // Advance source route and send on ulnid
        // Checked route before
        if let Some(route) = message.source_route_mut() {
            route.advance();
        }
        log::trace!(target: "forward_protocol_message", "Forwarding message {:?}", message);
        context
            .runtime_mut()
            .send_message(message, context.pn_table().deref());
        HandlingResult::Handled
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase for ForwardProtocolMessage<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::InsertionStrategy: InsertionStrategy<C::RoutingTable, C::PhysicalNeighborTable, BUCKET_SIZE>,
    C::PhysicalNeighborTable: UNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    type State = ReactiveUseCaseState;

    fn start(&mut self, _context: &C) -> Result<(), Self::Error> {
        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

impl<C, const BUCKET_SIZE: usize> EventHandler for ForwardProtocolMessage<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::InsertionStrategy: InsertionStrategy<C::RoutingTable, C::PhysicalNeighborTable, BUCKET_SIZE>,
    C::PhysicalNeighborTable: UNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    type Context = C;
    type Error = NeverError;
    type Value = HandlingResult;

    fn handle_event(
        &mut self,
        context: &C,
        event: UseCaseEvent,
    ) -> Result<Self::Value, Self::Error> {
        if let UseCaseEvent::Message(message, ulnid) = event {
            // todo make more efficient pls
            if let UnderlayNeighborSource::UnderlayNeighbor(ulnid) = ulnid {
                self.extract_message_info(context, message.clone(), ulnid);
            }

            Ok(self.handle_forwarding(context, message))
        } else {
            Ok(HandlingResult::NotHandled)
        }
    }
}
