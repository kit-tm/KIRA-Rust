use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::marker::PhantomData;
use std::num::NonZeroU8;
use std::ops::{Deref, DerefMut};
use tracing::{Level, instrument};

use crate::domain::{
    Contact, ContactState, InsertionStrategy, InsertionStrategyResult, Link, NodeId, NotVia,
    NotViaState, Path, RoutingTable, Timestamp, ULNTable, UnderlayNeighborId,
    UnderlayNeighborSource, VicinityGraph,
};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{
    CommonHeader, ErrorData, ProtocolMessage, ProtocolMessageKind, RTableData, ReqRspMessage,
    RouteUpdateActionType,
};
use crate::use_cases::{
    EventHandler, HandlingResult, NeverError, ReactiveUseCaseState, UseCase, UseCaseContext,
    UseCaseEvent, UseCaseRuntime,
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
/// The [UseCase] returns a result which shows if the message was already handled and forwarded.
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
    C::InsertionStrategy: InsertionStrategy<C::RoutingTable, C::UnderlayNeighborTable, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
    C::VicinityGraph: VicinityGraph,
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

    /// Extracts source information, inserts it into the [ULNTable] and [RoutingTable] and returns
    /// the extracted [Contact] information.
    fn extract_source_information(
        &self,
        context: &C,
        message: &ProtocolMessage,
        ulnid: UnderlayNeighborId,
    ) -> Option<Contact> {
        // insertion of underlay neighbors should only happen by ULNDiscReqRsp
        if let ProtocolMessage::ULNHello(_) = message {
            return None;
        }

        let path = self.extract_path_to_source(message);
        let ssn = message.source_state_seq_nr();
        if ssn.is_reset() {
            todo!("implement reset of StateSeqNr")
        }

        let ssn = ssn.value()?;
        let contact = Contact::new(path.clone(), ssn);

        {
            let mut uln_table = context.uln_table_mut();
            let neighbor_id = path.first();
            // loopback: the sender was us
            if neighbor_id == context.root_id() {
                return None;
            }

            if !uln_table.contains(neighbor_id) {
                if let Some(replaced) = uln_table.insert(*neighbor_id, ulnid) {
                    // Not allowed to happen as lock is held
                    log::warn!(
                        target: "un_table",
                        "Overwritten ulnid mapping for '{neighbor_id}' from '{ulnid}' to '{replaced}' but checked before"
                    );
                } else {
                    log::debug!(target: "un_table", "Inserted neighbor '{neighbor_id}' at ulnid '{ulnid}'");
                }
            }
        }

        self.update_contact(context, contact.clone());

        Some(contact)
    }

    /// Attempts to insert the contact into the routing table which may create a new entry,
    /// update an existing entry, or do nothing.
    ///
    /// If the contact was previously an underlay neighbor but not anymore its entry in the ULNTable
    /// will be removed.
    // TODO check notvia handling considering age
    fn update_contact(&self, context: &C, contact: Contact) {
        if let Some(not_via) = context
            .not_via_state()
            .iter()
            .find(|not_via| contact.path().contains_link(&not_via.link))
        {
            log::trace!(target: "forward_protocol_message", "Skipping contact - its path contains a not-via link {not_via}; {contact}");
            return;
        }

        // ignore information about us from other parties
        if contact.id() == context.root_id() {
            return;
        }

        log::trace!(target: "forward_protocol_message", "Attempting to insert {contact} into routing table");
        let result = context.routing_table_insertion_strategy().insert(
            contact.clone(),
            context.routing_table_mut().deref_mut(),
            context.uln_table().deref(),
        );

        // Remove if contact changed the routing table in any way, was an underlay neighbor and is not a ULN anymore
        if result != InsertionStrategyResult::Dropped
            && context.uln_table().contains(contact.id())
            && !context
                .routing_table()
                .contact(contact.id())
                .map(Contact::is_uln)
                .unwrap_or(false)
        {
            context.uln_table_mut().remove(contact.id());
            log::debug!(target: "forward_protocol_message", "Removed {} from ULNTable as it is no more an undelay neighbor; {:?}", contact.id(), contact);
        } else {
            log::debug!(target: "forward_protocol_message", "Routing table insertion result {:?}",result);
        }
    }

    fn extract_rtable_reqrsp(
        &self,
        context: &C,
        request: &ReqRspMessage<RTableData>,
        path_to_source: Path,
    ) {
        for mut contact in request.data.contacts.clone() {
            let mut path = path_to_source.clone();
            path.extend(contact.path().clone());
            *contact.path_mut() = path;

            self.update_contact(context, contact);
        }
    }

    fn extract_failed_contact(&self, context: &C, request: &ReqRspMessage<ErrorData>) {
        match &request.data {
            ErrorData::SegmentFailure {
                failed_link: link, ..
            } => {
                let mut not_via_state = context.not_via_state_mut();
                let mut routing_table = context.routing_table_mut();

                // probably update the timestamp
                let nvs_entry = NotViaState::new(link.clone(), Timestamp::now());
                not_via_state.replace(nvs_entry.clone());

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

    /// Extracts not_via data and applies them on the routing table.
    fn extract_not_via_data(
        &self,
        context: &C,
        source: &NodeId,
        new_not_via_data: &HashSet<NotVia>,
    ) {
        // we exclude any notvia that contains ourselves, because we know better
        let mut filtered_not_via_data = new_not_via_data
            .iter()
            .filter(|notvia| notvia.link.contains(context.root_id()));

        let mut routing_table = context.routing_table_mut();

        // invalidate contacts that have older path information than notvia and contain a notvia link
        for mut contact in routing_table.iter_mut() {
            let not_via_invalidation = filtered_not_via_data
                .find(|entry| {
                    *(contact.last_seen()) < Timestamp::from_age(entry.age)
                        && contact.path().contains_link(&entry.link)
                })
                .cloned();
            if not_via_invalidation.is_none() {
                continue;
            }
            *contact.state_mut() = ContactState::Invalid;

            log::debug!(target: "forward_protocol_message", "Invalidated contact {} based on not-via data of {}", contact.id(), source);
        }
    }

    fn handle_update_routes<I: IntoIterator<Item = (Contact, RouteUpdateActionType)>>(
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

        for (updated_contact, update_action) in route_updates {
            if let Some(mycontact) = routing_table.contact(updated_contact.id()) {
                // if updated contact is a ULN of this node, we ignore information about it
                if mycontact.is_uln() {
                    continue;
                }
            } else {
                // Skip updates for contacts which are not contained in own routing table
                continue;
            }

            let mut new_path = source_contact.path().clone();
            new_path.extend(updated_contact.path().clone());

            // Note that for all actions we have the contact already, checked for existence before
            match update_action {
                // Invalidate contact -> Every Contact affected by that will be handled in
                //                       FailureHandling use case
                RouteUpdateActionType::Unreachable => {
                    // this is only useful for ULN contacts to consider
                    let mut saved_contact =
                        routing_table.contact_mut(updated_contact.id()).unwrap();
                    if saved_contact.path().contains(source_id)
                        && saved_contact.is_older_than(&updated_contact)
                    {
                        *saved_contact.state_mut() = ContactState::Invalid;
                        log::trace!(target: "forward_protocol_message", "Invalidated contact {} based on route update data of {} [Removed]", saved_contact.id(), source_id);
                    }
                }
                RouteUpdateActionType::Announce | RouteUpdateActionType::Change => {
                    // Announce: Sender has the contact as new contact, but we know it already according to precondition above
                    // Change: Path has been changed, usually an improvement
                    // Probably update Path of contact if path is better and more recent
                    // TODO the path should only be used as proposed path that needs to be validated
                    let mut old_contact = routing_table.contact_mut(updated_contact.id()).unwrap();
                    if old_contact.path().size() > new_path.size()
                        && old_contact.path().contains(source_id)
                        && old_contact.is_older_than(&updated_contact)
                        && updated_contact.state() == &ContactState::Valid
                    {
                        *old_contact.state_mut() = ContactState::Valid;
                        log::trace!(target: "forward_protocol_message", "Update from {} for contact {} provides improved path {}", source_id, old_contact.id(), new_path);
                    }
                }
                RouteUpdateActionType::WithDraw => {
                    // no action right now
                }
            }
        }
    }

    /// Process meta-information of the received [ProtocolMessage].
    ///
    /// The processing happens at _every_ hop of the message.
    /// Message-specific actions at the destinations are handled by specialized [UseCases](UseCase).
    /// The information processed is:
    ///
    /// 1. [StateSeqNr](crate::domain::StateSeqNr) to note updates in the vicinity of the node.
    /// 2. [NotVia] to invalidate all effected [Contacts](Contact).
    /// 3. [Path] to the source of the message to update the [RoutingTable] and [ULNTable].
    /// 4. [RTableData] to discover, improve of fix existing [Contacts](Contact) in the [RoutingTable].
    fn extract_message_info(
        &self,
        context: &C,
        message: &ProtocolMessage,
        ulnid: UnderlayNeighborId,
    ) {
        // extract not via information
        if let Some(not_via) = message.not_via() {
            self.extract_not_via_data(context, message.source(), not_via);
        }

        // extract information from source route, potentially update the corresponding contact
        let source_contact = self.extract_source_information(context, message, ulnid);

        match message {
            // process messages with RTable information
            ProtocolMessage::ULNDiscReq(msg)
            | ProtocolMessage::ULNDiscRsp(msg)
            | ProtocolMessage::QueryRouteRsp(msg)
            | ProtocolMessage::FindNodeRsp(msg) => {
                if let Some(source_contact) = source_contact {
                    self.extract_rtable_reqrsp(context, msg, source_contact.path().clone())
                }
            }
            // process error information
            ProtocolMessage::Error(error_rsp) => self.extract_failed_contact(context, error_rsp),
            // These are already covered by source info extraction
            // Explicitly listing to yield compile time errors as soon as changes happen to ProtocolMessage enum
            ProtocolMessage::UpdateRouteReq(req) => {
                self.handle_update_routes(
                    context,
                    req.source_route.source(),
                    req.contact_actions.clone(),
                );
            }
            ProtocolMessage::ULNHello(_)
            | ProtocolMessage::QueryRouteReq(_)
            | ProtocolMessage::FindNodeReq(_)
            | ProtocolMessage::ProbeReq(_)
            | ProtocolMessage::ProbeRsp(_)
            | ProtocolMessage::PathSetupReq(_)
            | ProtocolMessage::PathTeardownReq(_)
            | ProtocolMessage::StoreReq(_)
            | ProtocolMessage::StoreRsp(_)
            | ProtocolMessage::FetchReq(_)
            | ProtocolMessage::FetchRsp(_) => {}
        }
    }

    fn handle_next_hop_failed(&self, context: &C, message: ProtocolMessage) {
        if message.msg_id().is_none() {
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
            common_header: CommonHeader::new(
                ProtocolMessageKind::Error,
                *context.root_id(),
                *message.source(),
                Some(message.msg_id().unwrap().into()),
                Some(From::from(*context.uln_table().state_seq_nr())),
                context.uln_table().size(),
            ),
            data: ErrorData::SegmentFailure {
                failed_link,
                source: root_id,
            },
            not_via: context.not_via_state().iter().map(NotVia::from).collect(),
            source_route: SourceRoute::from_reversed(message.source_route().unwrap().clone()),
        };

        context
            .runtime()
            .send_message(error_message, context.uln_table().deref());
    }

    /// Forwards the [ProtocolMessage] to the next hop.
    ///
    /// Returns if the message was forwarded.
    /// This information is used to abort the processing of forwarded messages
    /// by subsequent invoked [UseCases](UseCase).
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
                "Searching next overlay hop for message [{message:?}]"
            );

            // Overlay Routing
            // TODO: add unit tests
            // fixme isolated error if only a single node is used
            // fixme remove cycles in SourceRoute on the way back?
            // fixme respect NotVia? [lib/src/use_cases/handle_overlay_discovery.rs:141]
            let overlay_destination = overlay_destination.unwrap();

            // intended overlay destination is us -> nothing to forward
            if overlay_destination == context.root_id() {
                return HandlingResult::NotHandled;
            }

            // TODO: support other shared_prefix_grouping via config
            let closest_node = context
                .routing_table()
                .next_hop(overlay_destination, 20, NonZeroU8::MIN)
                .expect("Shared Prefix Grouping should be valid");

            // closest known overlay hop is us -> nothing to forward,
            if closest_node.is_none() {
                log::debug!(
                    target: "forward_protocol_message",
                    "Final destination of overlay message is us [{message:?}]"
                );
                return HandlingResult::NotHandled;
            }

            let next_contact = closest_node.unwrap();

            // extend source route to next hop
            if let Some(sr) = message.source_route_mut() {
                sr.extend(next_contact.path().clone())
            }
            next_hop = message.source_route().and_then(SourceRoute::next_hop);

            log::trace!(target: "forward_protocol_message", "Forwarding overlay message to next hop [{message:?}]");
        }
        let next_hop = next_hop.unwrap();

        // From here on the message is assumed to be for us

        // Check if next link is in not_via data
        // FIXME this should probably not be checked using NotViaState but using the neighbor table?!
        if context.not_via_state().contains(&NotViaState::new(
            Link::new(*context.root_id(), *next_hop),
            Timestamp::now(),
        )) {
            self.handle_next_hop_failed(context, message);
            return HandlingResult::Handled;
        }

        // Next hop is not a underlay neighbor -> Error -> Drop
        let neighbor_ulnid = context.uln_table().get(next_hop).cloned();
        if neighbor_ulnid.is_none() {
            self.handle_next_hop_failed(context, message);
            return HandlingResult::Handled;
        }

        // Advance source route and send on ulnid
        // Checked route before
        if let Some(route) = message.source_route_mut() {
            route.advance();
        }
        log::trace!(target: "forward_protocol_message", "Forwarding message {message:?}");
        context
            .runtime()
            .send_message(message, context.uln_table().deref());
        HandlingResult::Handled
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase for ForwardProtocolMessage<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::InsertionStrategy: InsertionStrategy<C::RoutingTable, C::UnderlayNeighborTable, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
    C::VicinityGraph: VicinityGraph,
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
    C::InsertionStrategy: InsertionStrategy<C::RoutingTable, C::UnderlayNeighborTable, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
    C::VicinityGraph: VicinityGraph,
{
    type Context = C;
    type Error = NeverError;
    type Value = HandlingResult;

    #[instrument(
        level = Level::TRACE,
        target = "forward_protocol_message",
        "forward_protocol_message",
        skip(self, context),
        fields(state = ?self.state)
    )]
    fn handle_event(
        &mut self,
        context: &C,
        event: UseCaseEvent,
    ) -> Result<Self::Value, Self::Error> {
        // this use case only handles message events
        if let UseCaseEvent::Message(message, ulnid) = event {
            // TODO: make more efficient pls
            if let UnderlayNeighborSource::UnderlayNeighbor(ulnid) = ulnid {
                // extract useful message info of bypassing messages (NotVia, RTable)
                self.extract_message_info(context, &message, ulnid);
            }

            Ok(self.handle_forwarding(context, message))
        } else {
            Ok(HandlingResult::NotHandled)
        }
    }
}
