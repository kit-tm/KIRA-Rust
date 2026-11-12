use std::{
    collections::{
        HashMap,
        HashSet,
    },
    fmt::Debug,
    marker::PhantomData,
    ops::{
        Deref,
        DerefMut,
    },
};

use tracing::{
    Level,
    instrument,
};

use crate::{
    domain::{
        Contact,
        ContactState,
        InsertionStrategy,
        Link,
        NodeId,
        NotVia,
        NotViaList,
        NotViaState,
        NotViaStateList,
        Path,
        ProtocolMessage,
        ProtocolMessageKind,
        RoutingTable,
        SourceRoute,
        Timestamp,
        ULNState,
        ULNTable,
        UnderlayNeighborId,
        VicinityGraph,
        insertion_strategy::InsertionStrategyResult,
        path::{
            InOrderCycleRemover,
            PathCycleRemover,
        },
        protocol_message::{
            CommonHeader,
            ErrorData,
            ProtocolMessageFlags,
            RTableData,
            ReqRspMessage,
            RouteUpdateActionType,
            WireFormatMessage as _,
        },
    },
    use_cases::{
        ContactEvent,
        EventHandler,
        HandlingResult,
        NeverError,
        ReactiveUseCaseState,
        UseCase,
        UseCaseContext,
        UseCaseEvent,
        UseCaseRuntime,
    },
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
enum ForwardingError {
    FwdErrIgnore,
    FwdErrDropMessage,
}

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
    C::UnderlayNeighborTable:
        ULNTable + Deref<Target = HashMap<NodeId, (UnderlayNeighborId, ULNState)>>,
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
        path.set_valid();
        path
    }

    /// Extracts source information, inserts it into the [ULNTable] and [RoutingTable] and returns
    /// the extracted [Contact] information.
    fn extract_source_information(
        &self,
        context: &C,
        message: &ProtocolMessage,
    ) -> Result<Contact, ForwardingError> {
        // insertion of underlay neighbors should only happen by ULNDiscReqRsp
        if let ProtocolMessage::ULNHello(_) = message {
            return Err(ForwardingError::FwdErrIgnore);
        }

        let mut path = self.extract_path_to_source(message);
        let ssn = message.source_state_seq_nr();
        let ssn = ssn.value().ok_or(ForwardingError::FwdErrIgnore)?;
        // source routes are automatically validated
        // however, if this is a queued message it's information may not be true anymore
        // so especially we ignore information about ULNs (this message may have been received after the interface went down)
        path.set_valid();
        // new automatically creates a Contact in Valid state but requires a valid path
        let contact = Contact::new(path.clone(), ssn);
        //log::trace!(target: "forward_protocol_message", "extract_source_information for contact {} = {}",contact.id(),contact);

        {
            let neighbor_id = path.first();
            // received somehow a loopback message: the sender was us
            if neighbor_id == context.root_id() {
                return Err(ForwardingError::FwdErrDropMessage);
            }

            // A message may have been queued before the neighbor's interface went down and only
            // be processed after the neighbor was removed from the ULNTable. Such a message
            // must not revalidate the contact, because the neighbor is unreachable.
            match message {
                ProtocolMessage::ULNDiscRsp(_) if !context.uln_table().contains(neighbor_id) => {
                    // ULNDiscReqs are only sent to neighbors in the ULNTable,
                    // so the response is outdated
                    log::debug!(target: "forward_protocol_message", "Received ULNDiscRsp from '{neighbor_id}' that is not in ULNTable – dropping outdated message");
                    return Err(ForwardingError::FwdErrDropMessage);
                }
                ProtocolMessage::ULNDiscReq(_) if !context.uln_table().contains(neighbor_id) => {
                    // either a new neighbor or an outdated request: only create a new contact,
                    // VicinityDiscovery inserts the neighbor and later messages update the contact
                    if context.routing_table().contains(neighbor_id) {
                        log::debug!(target: "forward_protocol_message", "Received ULNDiscReq from '{neighbor_id}' that is not in ULNTable – not updating its contact");
                        return Err(ForwardingError::FwdErrIgnore);
                    }
                }
                ProtocolMessage::ULNDiscReq(_) | ProtocolMessage::ULNDiscRsp(_) => {}
                _ => {
                    // this may happen in case the interface is currently disabled and the message arrived before
                    if !context.uln_table().contains(neighbor_id) {
                        log::debug!(target: "forward_protocol_message", "Received message from '{neighbor_id}' that is not in ULNTable – ignoring");
                        return Err(ForwardingError::FwdErrDropMessage);
                    }
                    // A tentative link is only known to work towards us, so the reversed path
                    // may not work. The message itself is still processed and forwarded.
                    // (ULNDiscReq/Rsp are excluded above: they prove the link works both ways.)
                    if !context.uln_table().is_bidirectional(neighbor_id) {
                        log::debug!(target: "forward_protocol_message", "Received message via tentative neighbor '{neighbor_id}' – not updating contact {}", contact.id());
                        return Ok(contact);
                    }
                }
            }
        }

        self.update_contact(context, contact.clone());

        Ok(contact)
    }

    /// Attempts to insert the contact into the routing table which may create a new entry,
    /// update an existing entry, or do nothing. This is called from extract_source_information()
    /// and extract_rtable_reqrsp().
    ///
    /// The ULNTable is not modified here: removal of underlay neighbors is handled by
    /// vicinity_discovery (ULN sync timeout) and failure_handling (interface down).
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

        match result {
            InsertionStrategyResult::NeedsValidation => {
                // if this is a new candidate, we need to check its validity first,
                // if it is an existing contact but only with a proposed path, we need to set the proposed path first
                log::trace!(target: "forward_protocol_message", "proposed contact will be checked later {:?} ", contact);

                // spawn probing event
                context
                    .runtime()
                    .broadcast_event(Box::new(ContactEvent::Proposed(contact)));
            }
            InsertionStrategyResult::Dropped => {
                log::trace!(target: "forward_protocol_message", "contact {} dismissed by RT insertion strategy", contact.id());
            }
            InsertionStrategyResult::Inserted
            | InsertionStrategyResult::Replaced(_)
            | InsertionStrategyResult::Updated => {
                log::debug!(target: "forward_protocol_message", "Routing table insertion result for contact {}: {:?} ", contact.id(), result);
            }
        }
    }

    /// Estimated time a message from `source` took to reach us: half of the smoothed
    /// round-trip time to it, or zero without measurement.
    ///
    /// It is added to received ages, as the information got older on the way.
    fn transit_delay(context: &C, source: &NodeId) -> std::time::Duration {
        context
            .routing_table()
            .one_way_delay(source)
            .unwrap_or_default()
    }

    fn extract_rtable_reqrsp(&self, context: &C, rtable: &RTableData, path_to_source: Path) {
        // the reported information got older on its way to us
        let delay = Self::transit_delay(context, path_to_source.last());
        for mut reported_contact in rtable.contacts.clone() {
            reported_contact.set_info_age(reported_contact.age().saturating_add(delay));
            let mut path = path_to_source.clone();
            if reported_contact.path().is_some() {
                path.extend(reported_contact.path().unwrap().clone());
            }
            // we must ignore any paths that lead via ourselves
            if path.contains(context.root_id()) {
                continue;
            }
            // we now have potential cycles that we need to remove
            InOrderCycleRemover.remove_cycles_in_place(&mut path);
            // since this is indirect information we heard, we do not trust it
            // NOTE: maybe we could trust information about ULNs from this note if fairly recently updated
            path.unset_last_validated();
            path.set_notvalidated();

            assert_eq!(reported_contact.id(), path.last());
            reported_contact.set_path(path);
            // only a validated path makes a contact valid
            reported_contact.set_state(ContactState::Unknown);
            log::debug!(target: "forward_protocol_message", "Contact from reading rtable: {:?} ", reported_contact);

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
            ErrorData::RouteFailureWrongHop {
                wrong_hop: link, ..
            } => {
                // this is an indication that there is an implementation error
                log::error!(target: "forward_protocol_messate", "RouteFailureWrongHop {:?}",link);
            }
        };
    }

    /// Extracts not_via data and applies them on the routing table.
    fn extract_not_via_data(&self, context: &C, source: &NodeId, new_not_via_data: &NotViaList) {
        // the failure information got older on its way to us
        let delay = Self::transit_delay(context, source);
        // we exclude any notvia that contains ourselves, because we know better
        let filtered_not_via_data = new_not_via_data
            .iter()
            .filter(|notvia| !notvia.link.contains(context.root_id()))
            .map(|notvia| NotVia::from((notvia.link.clone(), notvia.age.saturating_add(delay))))
            .collect::<Vec<NotVia>>();

        let mut routing_table = context.routing_table_mut();

        // invalidate contacts that have older path information than notvia and whose active path contain a notvia link
        for mut contact in routing_table
            .iter_mut()
            .filter(|c| c.is_valid() && c.path().is_some())
        {
            // collect all failed links on the active path before invalidating it,
            // because invalidate() refreshes last_validated
            let path = contact.path().unwrap();
            let Some(last_validated) = path.get_last_validated().copied() else {
                continue;
            };
            let failed: HashSet<NotViaState> = filtered_not_via_data
                .iter()
                .filter(|entry| {
                    last_validated < Timestamp::from(entry.age) && path.contains_link(&entry.link)
                })
                .map(|entry| NotViaState::from(entry.clone()))
                .collect();
            if failed.is_empty() {
                continue;
            }
            let failed_links = failed.len();
            contact.set_invalid(NotViaStateList::from(failed));
            tracing::debug!(target: "forward_protocol_message", %source, invalidated_contact = %contact.id(), failed_links, "Invalidated contact based on not-via data");
        }
    }

    fn handle_update_routes<I: IntoIterator<Item = (Contact, RouteUpdateActionType)>>(
        &self,
        context: &C,
        source_id: &NodeId,
        route_updates: I,
    ) {
        // the reported information got older on its way to us
        let delay = Self::transit_delay(context, source_id);
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
        for (mut updated_contact, update_action) in route_updates {
            updated_contact.set_info_age(updated_contact.age().saturating_add(delay));
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
            // paths should never contain ourselves
            if new_path.contains(context.root_id()) {
                continue;
            }
            // we now have potential cycles that we need to remove
            InOrderCycleRemover.remove_cycles_in_place(&mut new_path);
            // path is not yet validated, so we use the state to indicated this
            new_path.set_notvalidated();
            assert_eq!(updated_contact.id(), new_path.last());

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
                        // date the failure by the reported age, not by the time of receipt
                        affected_contact.set_invalid(NotViaStateList::from(NotViaState::new(
                            direct_uln_link,
                            *updated_contact.last_seen(),
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
                RouteUpdateActionType::Other(other_raw) => {
                    tracing::warn!(target: "forward_protocol_message", raw = ?other_raw, "Unsupported RouteUpdateActionType");
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
        skip(self, context),
    )]
    fn extract_message_info(
        &self,
        context: &C,
        message: &ProtocolMessage,
    ) -> Result<(), ForwardingError> {
        // extract not via information
        if let Some(not_via) = message.not_via() {
            self.extract_not_via_data(context, message.source(), not_via);
        }

        // extract information from source route, potentially update the corresponding contact
        let source_contact = self.extract_source_information(context, message)?;

        match message {
            // process messages with RTable information
            ProtocolMessage::ULNDiscReq(msg) | ProtocolMessage::ULNDiscRsp(msg) => {
                self.extract_rtable_reqrsp(
                    context,
                    &msg.data,
                    source_contact.path().unwrap().clone(),
                );
            }
            ProtocolMessage::QueryRouteRsp(msg) | ProtocolMessage::FindNodeRsp(msg) => {
                self.extract_rtable_reqrsp(
                    context,
                    &msg.data,
                    source_contact.path().unwrap().clone(),
                );
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
        Ok(())
    }

    fn handle_next_hop_failed(&self, context: &C, message: ProtocolMessage) {
        if message.msg_id().is_none()
            && !message
                .msg_flags()
                .contains(ProtocolMessageFlags::Diagnostic)
        {
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
                Some(message.msg_id().unwrap()),
                *context.uln_table().state_seq_nr(),
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
        let Some(source_route) = message.source_route_mut() else {
            // ULNHello messages have no source route and need no forwarding
            return HandlingResult::NotHandled;
        };
        let span = tracing::Span::current();
        if !span.is_disabled() {
            span.record("current_hop", format!("{}", source_route.current_hop()));
        }

        // Current hop has to be us
        if source_route.current_hop() != context.root_id() {
            tracing::error!(
                target: "forward_protocol_message",
                "Current hop of message {} is not us",
                source_route.current_hop(),
            );
            return HandlingResult::Handled;
        }

        // touch last_seen value of ULN that forwarded the message to us
        if let Some(entry) = context
            .vicinity_graph_mut()
            .entry_mut(source_route.prev_hop())
        {
            entry.update_last_seen(context.runtime().current_time());
        } else {
            tracing::error!(
                target: "forward_protocol_message",
                "no previous hop found in vicinity graph",
            );
        };

        let Some(next_hop) = source_route.next_hop() else {
            tracing::debug!(
                target: "forward_protocol_message",
                "SourceRoute finished",
            );
            return HandlingResult::NotHandled;
        };

        // From here on the message is assumed to be for us

        // Next hop is not a underlay neighbor -> Error -> Drop
        // TODO distinguish between failed next hop and wrong (non-existing hop ), see RouteFailureWrongHop
        if context.uln_table().get(next_hop).is_none() {
            tracing::warn!(
                target: "forward_protocol_message",
                reason = "Next hop is not an underlay neighbor",
                ?message,
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
    C::UnderlayNeighborTable:
        ULNTable + Deref<Target = HashMap<NodeId, (UnderlayNeighborId, ULNState)>>,
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
    C::UnderlayNeighborTable:
        ULNTable + Deref<Target = HashMap<NodeId, (UnderlayNeighborId, ULNState)>>,
    C::VicinityGraph: VicinityGraph,
{
    type Context = C;
    type Error = NeverError;
    type Value = HandlingResult;

    #[instrument(
        target = "forward_protocol_message",
        "forward_protocol_message",
        skip(self, context, event),
        fields(state = ?self.state)
        ret(level = Level::TRACE),
    )]
    fn handle_event(
        &mut self,
        context: &C,
        event: UseCaseEvent,
    ) -> Result<Self::Value, Self::Error> {
        // this use case only handles message events
        if let UseCaseEvent::Message(message, _uln_source) = event {
            if let Some(source_route) = message.source_route() {
                // Current hop has to be us, do not use misrouted packets
                // to extract information
                if source_route.current_hop() != context.root_id() {
                    tracing::error!(
                        target: "forward_protocol_message",
                        "Current hop of message {} is not us",
                        source_route.current_hop(),
                    );
                    return Ok(HandlingResult::Handled);
                }

                // check if current underlay neighbor is still up
                // as interface_down events delete the ulnid from the uln_table
                if !context.uln_table().contains_key(source_route.prev_hop()) {
                    tracing::error!(
                        target: "forward_protocol_message",
                        "Message arrived from ULN {} that is currently not available – will drop {:?}",
                        source_route.prev_hop(),
                        message,
                    );
                    return Ok(HandlingResult::Handled);
                }
            }

            // extract useful message info of bypassing messages (NotVia, RTable)
            if let Err(ForwardingError::FwdErrDropMessage) =
                self.extract_message_info(context, &message)
            {
                return Ok(HandlingResult::Handled);
            }

            Ok(self.handle_forwarding(context, message))
        } else {
            Ok(HandlingResult::NotHandled)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        context::{
            ContextConfig,
            SyncContext,
        },
        domain::{
            ConnectionId,
            InterfaceId,
            Nonce,
            SafeStateSeqNr,
            StateSeqNr,
            UnderlayNeighborSource,
            insertion_strategy::UNSStrategy,
            path::ShortestFirstPathSimplifier,
            protocol_message::{
                FindNodeReqData,
                ULNReqRspMessage,
            },
            routing_table::SingleBucketRT,
            underlay_neighbor_table::InMemoryULNTable,
            vicinity::vicinity_graph::PetVicinityGraph,
        },
        runtime::testing::TestingUseCaseRuntime,
    };

    const BUCKET_SIZE: usize = 20;
    type RT = SingleBucketRT<BUCKET_SIZE>;
    type Ctx = SyncContext<
        RT,
        TestingUseCaseRuntime,
        UNSStrategy<RT, InOrderCycleRemover, ShortestFirstPathSimplifier, BUCKET_SIZE>,
        InMemoryULNTable,
        PetVicinityGraph,
    >;

    fn root() -> NodeId {
        NodeId::with_msb(1)
    }
    fn neighbor() -> NodeId {
        NodeId::with_msb(2)
    }
    fn ulnid() -> UnderlayNeighborId {
        UnderlayNeighborId {
            interface_id: InterfaceId::try_from(1).unwrap(),
            connection_id: ConnectionId(0),
        }
    }
    fn ssn(value: u32) -> SafeStateSeqNr {
        SafeStateSeqNr::try_from(value).unwrap()
    }

    fn context() -> Ctx {
        crate::tests::init();
        Ctx::new(ContextConfig {
            root_id: root(),
            routing_table: RT::new(root()),
            runtime: TestingUseCaseRuntime::default(),
            insertion_strategy: UNSStrategy::new(InOrderCycleRemover, ShortestFirstPathSimplifier),
            uln_table: InMemoryULNTable::new(),
            vicinity_graph: PetVicinityGraph::new(root()),
        })
    }

    /// neighbor contact with the direct path, invalidated as after its interface went down
    fn insert_invalidated_neighbor(ctx: &Ctx) {
        let mut rt = ctx.routing_table_mut();
        rt.insert(Contact::new(Path::from(neighbor()), ssn(1)))
            .unwrap();
        rt.contact_mut(&neighbor())
            .unwrap()
            .set_invalid(NotViaStateList::from(NotViaState::new(
                Link::new(root(), neighbor()),
                Timestamp::now(),
            )));
    }

    fn uln_disc_message(kind: ProtocolMessageKind, neighbor_ssn: u32) -> ProtocolMessage {
        let message = ULNReqRspMessage {
            common_header: CommonHeader::new(
                kind,
                neighbor(),
                root(),
                Some(Nonce::random()),
                StateSeqNr::Value(ssn(neighbor_ssn)),
                0,
            ),
            data: RTableData { contacts: vec![] },
        };
        match kind {
            ProtocolMessageKind::ULNDiscReq => ProtocolMessage::ULNDiscReq(message),
            ProtocolMessageKind::ULNDiscRsp => ProtocolMessage::ULNDiscRsp(message),
            _ => unreachable!(),
        }
    }

    fn receive(ctx: &Ctx, message: ProtocolMessage) -> HandlingResult {
        ForwardProtocolMessage::<Ctx, BUCKET_SIZE>::default()
            .handle_event(
                ctx,
                UseCaseEvent::Message(message, UnderlayNeighborSource::UnderlayNeighbor(ulnid())),
            )
            .unwrap()
    }

    fn neighbor_contact(ctx: &Ctx) -> Option<Contact> {
        ctx.routing_table().contact(&neighbor()).cloned()
    }

    #[test]
    fn outdated_uln_disc_rsp_of_removed_neighbor_is_dropped() {
        let _clock = crate::tests::TestClock::new();
        let ctx = context();
        insert_invalidated_neighbor(&ctx);

        let result = receive(&ctx, uln_disc_message(ProtocolMessageKind::ULNDiscRsp, 2));

        assert!(
            matches!(result, HandlingResult::Handled),
            "not passed to other use cases"
        );
        assert!(
            neighbor_contact(&ctx).unwrap().is_invalid(),
            "contact stays invalid"
        );
    }

    #[test]
    fn uln_disc_req_does_not_revalidate_contact_of_removed_neighbor() {
        let _clock = crate::tests::TestClock::new();
        let ctx = context();
        insert_invalidated_neighbor(&ctx);

        let result = receive(&ctx, uln_disc_message(ProtocolMessageKind::ULNDiscReq, 2));

        assert!(
            matches!(result, HandlingResult::NotHandled),
            "VicinityDiscovery still has to process the request"
        );
        assert!(
            neighbor_contact(&ctx).unwrap().is_invalid(),
            "contact stays invalid"
        );
    }

    #[test]
    fn uln_disc_req_of_new_neighbor_creates_contact() {
        let _clock = crate::tests::TestClock::new();
        let ctx = context();

        let result = receive(&ctx, uln_disc_message(ProtocolMessageKind::ULNDiscReq, 1));

        assert!(matches!(result, HandlingResult::NotHandled));
        let contact = neighbor_contact(&ctx).expect("contact created");
        assert!(contact.is_valid());
        assert!(contact.is_uln());
    }

    #[test]
    fn message_with_reserved_ssn_creates_no_contact() {
        let _clock = crate::tests::TestClock::new();
        let ctx = context();
        let mut message = uln_disc_message(ProtocolMessageKind::ULNDiscReq, 1);
        let ProtocolMessage::ULNDiscReq(ref mut req) = message else {
            unreachable!()
        };
        // the reserved value 0xffffffff is decoded as invalid
        req.common_header = CommonHeader::new(
            ProtocolMessageKind::ULNDiscReq,
            neighbor(),
            root(),
            Some(Nonce::random()),
            StateSeqNr::from(u32::MAX),
            0,
        );

        receive(&ctx, message);

        assert!(neighbor_contact(&ctx).is_none());
    }

    #[test]
    fn uln_disc_rsp_of_known_neighbor_revalidates_contact() {
        let _clock = crate::tests::TestClock::new();
        let ctx = context();
        insert_invalidated_neighbor(&ctx);
        ctx.uln_table_mut().insert(neighbor(), ulnid());

        let result = receive(&ctx, uln_disc_message(ProtocolMessageKind::ULNDiscRsp, 2));

        assert!(matches!(result, HandlingResult::NotHandled));
        let contact = neighbor_contact(&ctx).unwrap();
        assert!(contact.is_valid(), "contact revalidated");
        assert!(contact.is_uln());
    }

    fn bidirectional(ctx: &Ctx, id: NodeId) {
        let mut uln_table = ctx.uln_table_mut();
        uln_table.insert(id, ulnid());
        uln_table.confirm(id, ulnid());
    }

    fn tentative(ctx: &Ctx, id: NodeId) {
        ctx.uln_table_mut().insert(id, ulnid());
    }

    /// A FindNodeReq from `route[0]` that traveled along `route` and just arrived at root.
    fn routed_message(route: &[NodeId]) -> ProtocolMessage {
        let source = route[0];
        let mut path: Vec<NodeId> = route[1..].to_vec();
        path.push(root());
        let mut source_route = SourceRoute::new(source, Path::try_from(path.as_slice()).unwrap());
        for _ in 1..route.len() {
            source_route.advance();
        }
        assert_eq!(source_route.current_hop(), &root());

        ProtocolMessage::FindNodeReq(ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::FindNodeReq,
                source,
                root(),
                Some(Nonce::random()),
                StateSeqNr::Value(ssn(1)),
                0,
            ),
            data: FindNodeReqData {
                neighborhood: std::num::NonZeroU64::new(1).unwrap(),
            },
            not_via: None,
            source_route,
        })
    }

    fn source() -> NodeId {
        NodeId::with_msb(5)
    }

    #[test]
    fn message_via_bidirectional_neighbor_creates_contact() {
        let _clock = crate::tests::TestClock::new();
        let ctx = context();
        bidirectional(&ctx, neighbor());

        receive(&ctx, routed_message(&[source(), neighbor()]));

        let contact = ctx
            .routing_table()
            .contact(&source())
            .cloned()
            .expect("contact created");
        assert!(contact.is_valid());
        assert_eq!(
            contact
                .path()
                .unwrap()
                .clone()
                .into_iter()
                .collect::<Vec<_>>(),
            [neighbor(), source()]
        );
    }

    #[test]
    fn message_via_tentative_neighbor_does_not_create_contact() {
        let _clock = crate::tests::TestClock::new();
        // the link to the neighbor may only work towards us (e.g. asymmetric radio)
        let ctx = context();
        tentative(&ctx, neighbor());

        let result = receive(&ctx, routed_message(&[source(), neighbor()]));

        assert!(
            matches!(result, HandlingResult::NotHandled),
            "message is still passed on"
        );
        assert!(!ctx.routing_table().contains(&source()));
        assert!(neighbor_contact(&ctx).is_none());
    }

    #[test]
    fn path_is_not_shortened_via_tentative_neighbor() {
        let _clock = crate::tests::TestClock::new();
        // message from S over <S, B, Q, P, root>, B only heard from (tentative)
        let ctx = context();
        let (p, q, b) = (NodeId::with_msb(3), NodeId::with_msb(4), neighbor());
        bidirectional(&ctx, p);
        tentative(&ctx, b);

        receive(&ctx, routed_message(&[source(), b, q, p]));

        let contact = ctx
            .routing_table()
            .contact(&source())
            .cloned()
            .expect("contact created");
        assert_eq!(
            contact
                .path()
                .unwrap()
                .clone()
                .into_iter()
                .collect::<Vec<_>>(),
            [p, q, b, source()],
            "path must not be shortened to <B, S>"
        );
    }

    #[test]
    fn path_shortened_via_bidirectional_neighbor_is_only_proposed() {
        let _clock = crate::tests::TestClock::new();
        // the traversed path is validated, the shortcut <B, S> is not
        let ctx = context();
        let (p, q, b) = (NodeId::with_msb(3), NodeId::with_msb(4), neighbor());
        bidirectional(&ctx, p);
        bidirectional(&ctx, b);

        receive(&ctx, routed_message(&[source(), b, q, p]));

        let contact = ctx
            .routing_table()
            .contact(&source())
            .cloned()
            .expect("contact created");
        let ids = |path: &Path| path.clone().into_iter().collect::<Vec<_>>();

        let active = contact.path().unwrap();
        assert_eq!(ids(active), [p, q, b, source()], "traversed path is used");
        assert!(active.is_valid());

        let proposed = contact.proposed_path().expect("shortcut is proposed");
        assert_eq!(ids(proposed), [b, source()]);
        assert!(
            proposed.is_notvalidated(),
            "shortcut has to be probed first"
        );
    }

    fn with_not_via(mut message: ProtocolMessage, link: Link, age_ms: u64) -> ProtocolMessage {
        let ProtocolMessage::FindNodeReq(ref mut req) = message else {
            unreachable!("routed_message creates FindNodeReqs");
        };
        req.not_via = Some(NotViaList::from([NotVia::from((
            link,
            crate::domain::Age::from(age_ms),
        ))]));
        message
    }

    /// Contact T over <neighbor, T>, the link <neighbor, T> is reported as failed afterwards.
    fn stale_failure_setup() -> (Ctx, NodeId, Link) {
        let ctx = context();
        bidirectional(&ctx, neighbor());
        let t = NodeId::with_msb(6);
        receive(&ctx, routed_message(&[t, neighbor()]));
        assert!(ctx.routing_table().contact(&t).unwrap().is_valid());
        (ctx, t, Link::new(neighbor(), t))
    }

    #[test]
    fn current_not_via_invalidates_contact() {
        let mut clock = crate::tests::TestClock::new();
        let (ctx, t, link) = stale_failure_setup();
        clock.advance(std::time::Duration::from_millis(10));

        // the link failed after the path to T was validated
        receive(
            &ctx,
            with_not_via(routed_message(&[source(), neighbor()]), link, 0),
        );

        assert!(ctx.routing_table().contact(&t).unwrap().is_invalid());
    }

    #[test]
    fn all_failed_links_on_the_path_are_recorded() {
        let mut clock = crate::tests::TestClock::new();
        let ctx = context();
        bidirectional(&ctx, neighbor());
        let x = NodeId::with_msb(5);
        let t = NodeId::with_msb(6);
        // T over <neighbor, X, T>
        receive(&ctx, routed_message(&[t, x, neighbor()]));
        assert!(ctx.routing_table().contact(&t).unwrap().is_valid());
        clock.advance(std::time::Duration::from_millis(10));

        // both links on the path to T failed after it was validated
        let mut message = routed_message(&[source(), neighbor()]);
        let ProtocolMessage::FindNodeReq(ref mut req) = message else {
            unreachable!("routed_message creates FindNodeReqs");
        };
        req.not_via = Some(NotViaList::from([
            NotVia::from((Link::new(neighbor(), x), crate::domain::Age::from(0))),
            NotVia::from((Link::new(x, t), crate::domain::Age::from(0))),
        ]));
        receive(&ctx, message);

        let rt = ctx.routing_table();
        let contact = rt.contact(&t).unwrap();
        let ContactState::Invalid(ref nvs) = *contact.state() else {
            panic!("contact should be invalid, is {:?}", contact.state());
        };
        let links: HashSet<Link> = nvs.nvs_list.iter().map(|nvs| nvs.link.clone()).collect();
        assert_eq!(
            links,
            HashSet::from([Link::new(neighbor(), x), Link::new(x, t)])
        );
    }

    #[test]
    fn outdated_not_via_is_ignored() {
        let _clock = crate::tests::TestClock::new();
        let (ctx, t, link) = stale_failure_setup();

        // the failure is older than the validation of the path to T
        receive(
            &ctx,
            with_not_via(routed_message(&[source(), neighbor()]), link, 60_000),
        );

        assert!(ctx.routing_table().contact(&t).unwrap().is_valid());
    }

    #[test]
    fn not_via_older_than_revalidation_by_traffic_is_ignored() {
        let mut clock = crate::tests::TestClock::new();
        // F7: the link failed, but the path to T was used again afterwards
        let (ctx, t, link) = stale_failure_setup();
        clock.advance(std::time::Duration::from_millis(60));
        // traffic from T over the same path revalidates it
        receive(&ctx, routed_message(&[t, neighbor()]));

        // the failure happened 30ms ago: after the first, but before the second validation
        receive(
            &ctx,
            with_not_via(routed_message(&[source(), neighbor()]), link, 30),
        );

        assert!(ctx.routing_table().contact(&t).unwrap().is_valid());
    }

    /// UpdateRouteReq from the neighbor reporting its link to `z` as unreachable.
    fn unreachable_update(z: NodeId, age_ms: u64) -> ProtocolMessage {
        let mut reported = Contact::new(Path::from(z), ssn(1));
        reported.set_info_age(crate::domain::Age::from(age_ms));
        ProtocolMessage::UpdateRouteReq(crate::domain::protocol_message::UpdateRouteReq {
            common_header: CommonHeader::new(
                ProtocolMessageKind::UpdateRouteReq,
                neighbor(),
                root(),
                None,
                StateSeqNr::Value(ssn(1)),
                0,
            ),
            not_via: None,
            contact_actions: vec![(
                reported,
                crate::domain::protocol_message::RouteUpdateActionType::Unreachable,
            )],
            source_route: SourceRoute::new(neighbor(), Path::from(root())),
        })
    }

    fn contact_behind_neighbor() -> (Ctx, NodeId) {
        let ctx = context();
        bidirectional(&ctx, neighbor());
        let z = NodeId::with_msb(7);
        receive(&ctx, routed_message(&[z, neighbor()]));
        assert!(ctx.routing_table().contact(&z).unwrap().is_valid());
        (ctx, z)
    }

    #[test]
    fn current_unreachable_update_invalidates_contact() {
        let mut clock = crate::tests::TestClock::new();
        let (ctx, z) = contact_behind_neighbor();
        // ages are compared in ms, the report has to be newer than the contact information
        clock.advance(std::time::Duration::from_millis(5));

        receive(&ctx, unreachable_update(z, 0));

        assert!(ctx.routing_table().contact(&z).unwrap().is_invalid());
    }

    #[test]
    fn outdated_unreachable_update_is_ignored() {
        let _clock = crate::tests::TestClock::new();
        // the report is older than what we know about the contact
        let (ctx, z) = contact_behind_neighbor();

        receive(&ctx, unreachable_update(z, 60_000));

        assert!(ctx.routing_table().contact(&z).unwrap().is_valid());
    }

    #[test]
    fn unreachable_update_from_before_a_reset_is_ignored() {
        let mut clock = crate::tests::TestClock::new();
        let (ctx, z) = contact_behind_neighbor();
        clock.advance(std::time::Duration::from_secs(5));
        // z restarted and its answer was accepted
        ctx.routing_table_mut()
            .contact_mut(&z)
            .unwrap()
            .reset_state_seq_nr(ssn(2));

        // reported by a node that still knew z from before its restart (3s ago)
        let mut update = unreachable_update(z, 3_000);
        let ProtocolMessage::UpdateRouteReq(ref mut req) = update else {
            unreachable!()
        };
        req.contact_actions[0].0.set_state_seq_nr(ssn(9));
        receive(&ctx, update);

        assert!(ctx.routing_table().contact(&z).unwrap().is_valid());
    }

    /// Creates the neighbor's own contact and sets its smoothed RTT.
    fn neighbor_with_srtt(ctx: &Ctx, srtt_ms: u64) {
        receive(ctx, routed_message(&[neighbor()]));
        ctx.routing_table_mut()
            .contact_mut(&neighbor())
            .expect("neighbor contact")
            .path_mut()
            .unwrap()
            .record_rtt(std::time::Duration::from_millis(srtt_ms));
    }

    #[test]
    fn not_via_without_rtt_estimate_is_current() {
        let mut clock = crate::tests::TestClock::new();
        let (ctx, t, link) = stale_failure_setup();
        receive(&ctx, routed_message(&[neighbor()]));
        clock.advance(std::time::Duration::from_millis(60));

        // failure 30ms ago, after the path to T was validated
        receive(&ctx, with_not_via(routed_message(&[neighbor()]), link, 30));

        assert!(ctx.routing_table().contact(&t).unwrap().is_invalid());
    }

    #[test]
    fn transit_delay_ages_not_via() {
        let mut clock = crate::tests::TestClock::new();
        let (ctx, t, link) = stale_failure_setup();
        // 100ms one-way delay from the neighbor
        neighbor_with_srtt(&ctx, 200);
        clock.advance(std::time::Duration::from_millis(60));

        // reported 30ms old, so 130ms old here: before the path to T was validated
        receive(&ctx, with_not_via(routed_message(&[neighbor()]), link, 30));

        assert!(ctx.routing_table().contact(&t).unwrap().is_valid());
    }

    #[test]
    fn transit_delay_ages_route_updates() {
        let mut clock = crate::tests::TestClock::new();
        let (ctx, z) = contact_behind_neighbor();
        neighbor_with_srtt(&ctx, 200);
        clock.advance(std::time::Duration::from_millis(5));

        // reported as current, but 100ms old here: older than the contact information
        receive(&ctx, unreachable_update(z, 0));

        assert!(ctx.routing_table().contact(&z).unwrap().is_valid());
    }
}
