use std::collections::HashMap;
use std::fmt::Debug;

use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use tracing::{Level, instrument};

use crate::domain::{
    Contact, InOrderCycleRemover, InsertionStrategy, InsertionStrategyResult, Link, NodeId, NotVia,
    NotViaList, NotViaState, NotViaStateList, Path, PathCycleRemover, PathState, RoutingTable,
    Timestamp, ULNTable, UnderlayNeighborId, UnderlayNeighborSource, VicinityGraph,
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
        // this path is validated because it was taken from a source route that has been traversed recently
        path.set_state(PathState::Valid);
        path.update_last_validated();
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
        // new automatically creates a Contact in Valid state
        let contact = Contact::new(path.clone(), ssn);
        //log::trace!(target: "forward_protocol_message", "extract_source_information for contact {} = {}",contact.id(),contact);

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
                        target: "uln_table",
                        "Overwritten ulnid mapping for '{neighbor_id}' from '{ulnid}' to '{replaced}' but checked before"
                    );
                } else {
                    log::debug!(target: "uln_table", "Inserted neighbor '{neighbor_id}' at ulnid '{ulnid}'");
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
    fn update_contact(&self, context: &C, contact: Contact) {
        // ignore information about us from other parties
        if contact.id() == context.root_id() {
            return;
        }

        // try to insert or update the contact in the routing table
        log::trace!(target: "forward_protocol_message", "Attempting to insert contact {} into routing table, contact: {}",contact.id(),contact);
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
            log::debug!(target: "forward_protocol_message", "Routing table insertion result for contact {}: {:?} ", contact.id(), result);
        }
    }

    fn extract_rtable_reqrsp(
        &self,
        context: &C,
        request: &ReqRspMessage<RTableData>,
        path_to_source: Path,
    ) {
        for mut reported_contact in request.data.contacts.clone() {
            let mut path = path_to_source.clone();
            if reported_contact.path().is_some() {
                path.extend(reported_contact.path().unwrap().clone());
            }
            // we now have potential cycles that we need to remove
            InOrderCycleRemover.remove_cycles_in_place(&mut path);
            path.set_state(PathState::Checking);
            path.unset_last_validated();
            reported_contact.set_path(path);

            self.update_contact(context, reported_contact);
        }
    }

    fn extract_failed_contact(&self, context: &C, request: &ReqRspMessage<ErrorData>) {
        match &request.data {
            ErrorData::SegmentFailure {
                failed_link: link, ..
            } => {
                // we ignore any weird message that includes ourselves in notvia information
                if link.contains(context.root_id()) {
                    return;
                }
                let mut routing_table = context.routing_table_mut();

                // a segment failure is recent (minus RTT/2), probably update the timestamp
                let nvs_entry = NotViaState::new(link.clone(), Timestamp::now());

                // check for all contacts that have an active path that is affected by the failed link
                for mut contact in routing_table.iter_mut() {
                    if let Some(active_path) = contact.path_mut()
                        && active_path.contains_link(link)
                    {
                        // invalidate path and contact now
                        contact.set_invalid(NotViaStateList::from(nvs_entry.clone()));
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
    fn extract_not_via_data(&self, context: &C, source: &NodeId, new_not_via_data: &NotViaList) {
        // we exclude any notvia that contains ourselves, because we know better
        let filtered_not_via_data = new_not_via_data
            .iter()
            .filter(|notvia| !notvia.link.contains(context.root_id()))
            .collect::<Vec<&NotVia>>();

        let mut routing_table = context.routing_table_mut();

        // invalidate contacts that have older path information than notvia and whose active path contain a notvia link
        for mut contact in routing_table
            .iter_mut()
            .filter(|c| c.is_valid() && c.path().is_some())
        {
            //let active_path = ;
            for entry in &filtered_not_via_data {
                if let Some(last_validated) = contact.path().unwrap().get_last_validated()
                    && *last_validated < Timestamp::from(entry.age)
                    && contact.path().unwrap().contains_link(&entry.link)
                {
                    contact
                        .set_invalid(NotViaStateList::from(NotViaState::from((**entry).clone())));
                    tracing::debug!(target: "forward_protocol_message", %source, invalidated_contact = %contact.id(), "Invalidated contact based on not-via data");
                    continue;
                }
            }
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
        // TODO using the source route of the message is probably better
        let Some(path_to_source_contact) = source_contact.path() else {
            return;
        };

        // iterate over all given contacts in the update message
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

            // this should not happen normally, but skip any None contact
            if updated_contact.path().is_none() {
                continue;
            }

            // concatenate paths to source_contact with path from it to the updated_contact
            let mut new_path = path_to_source_contact.clone();
            new_path.extend(updated_contact.path().unwrap().clone());
            // we now have potential cycles that we need to remove
            InOrderCycleRemover.remove_cycles_in_place(&mut new_path);
            // path is not yet validated, so we use the state to indicated this
            new_path.set_state(PathState::Checking);

            // Note that for all actions we have the contact already, checked for existence before
            match update_action {
                // Invalidate contact -> Every Contact affected by that will be handled in
                //                       FailureHandling use case
                RouteUpdateActionType::Unreachable => {
                    // this is only useful for ULN contacts of the source node to consider
                    let mut affected_contact = routing_table
                        .contact_mut(updated_contact.id())
                        .expect("contact should be present");
                    let direct_uln_link = Link::new(*source_id, *updated_contact.id());
                    if let Some(affected_path) = affected_contact.path()
                        && affected_path.contains_link(&direct_uln_link)
                        && affected_contact.is_older_than(&updated_contact)
                    {
                        affected_contact.set_invalid(NotViaStateList::from(NotViaState::new(
                            direct_uln_link,
                            Timestamp::now(),
                        )));
                        log::trace!(target: "forward_protocol_message", "Invalidated contact {} based on route update (Unreachable) of {} [Removed]", affected_contact.id(), source_id);
                    }
                }
                RouteUpdateActionType::Announce | RouteUpdateActionType::Change => {
                    // Announce: Sender has the contact as new contact, but we know it already according to precondition above
                    // Change: Path has been changed, usually an improvement
                    // Updates Proposed Path of contact if path is better and more recent (new_path is not validated)
                    let mut old_contact = routing_table.contact_mut(updated_contact.id()).unwrap();
                    if old_contact.assess_path_candidate_and_update(&new_path) {
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
    #[instrument(
        level = Level::DEBUG,
        target = "forward_protocol_message",
        skip_all,
        fields(message)
    )]
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
                    self.extract_rtable_reqrsp(context, msg, source_contact.path().unwrap().clone())
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
            not_via: None,
            source_route: SourceRoute::from_reversed(message.source_route().unwrap().clone()),
        };

        context.runtime().send_message(
            error_message,
            context.uln_table().deref(),
            context.root_id(),
        );
    }

    /// Forwards the [ProtocolMessage] to the next hop.
    ///
    /// Returns if the message was forwarded.
    /// This information is used to abort the processing of forwarded messages
    /// by subsequent invoked [UseCases](UseCase).
    #[instrument(
        level = Level::DEBUG,
        target = "forward_protocol_message",
        skip_all,
        fields(
            message,
            current_hop = tracing::field::Empty,
        )
        ret(level = Level::TRACE),
    )]
    fn handle_forwarding(&self, context: &C, mut message: ProtocolMessage) -> HandlingResult {
        let source_route = message.source_route_mut();
        if source_route.is_none() {
            return HandlingResult::NotHandled;
        }
        let source_route = source_route.unwrap();
        let span = tracing::Span::current();
        if !span.is_disabled() {
            span.record("current_hop", format!("{}", source_route.current_hop()));
        }

        // Current hop has to be us
        if source_route.current_hop() != context.root_id() {
            tracing::warn!(
                target: "forward_protocol_message",
                "Current hop of message is not us",
            );
            return HandlingResult::Handled;
        }

        let Some(next_hop) = source_route.next_hop() else {
            tracing::debug!(
                target: "forward_protocol_message",
                "SourceRoute finished",
            );
            return HandlingResult::NotHandled;
        };

        // From here on the message is assumed to be for us

        // Next hop is not a underlay neighbor -> Error -> Drop
        if context.uln_table().get(next_hop).is_none() {
            tracing::debug!(
                target: "forward_protocol_message",
                reason = "Next hop is not an underlay neighbor",
                "Dropping message and returning an error"
            );
            self.handle_next_hop_failed(context, message);
            return HandlingResult::Handled;
        }

        // Advance source route
        tracing::debug!(target: "forward_protocol_message", %next_hop, "Forwarding message to next hop");
        source_route.advance();
        context
            .runtime()
            .send_message(message, context.uln_table().deref(), context.root_id());
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
