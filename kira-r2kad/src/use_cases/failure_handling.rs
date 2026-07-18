use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::num::{NonZeroU8, NonZeroU32, NonZeroU64, NonZeroUsize};
use std::ops::Deref;
use std::time::Duration;
use tracing::{Level, instrument};

use derive_more::derive::{Display, Error};

use crate::domain::{
    Contact, ContactState, Link, NodeId, NotViaState, NotViaStateList, RoutingTable, Timestamp,
    ULNTable, UnderlayNeighborId, UnderlayNeighborUpdate, VicinityGraph,
};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{
    CommonHeader, ErrorData, FindNodeReqData, Nonce, ProtocolMessage, ProtocolMessageKind,
    ReqRspMessage, RouteUpdateActionType, UpdateRouteReq, WireFormatMessage,
};
use crate::use_cases::{
    ContactEvent, EventHandler, ReactiveUseCaseState, TimerId, UseCase, UseCaseContext,
    UseCaseEvent, UseCaseRuntime,
};
use crate::utils::InflightReqMap;
use crate::utils::errors::InsertionError;
use crate::utils::rediscovery_timeout_interval::RediscoveryTimeoutInterval;

const REDISCOVERY_WAITTIME_ULN: Duration = Duration::from_millis(100);
const REDISCOVERY_WAITTIME_CLOSEST_NEIGHBORS: Duration = Duration::from_millis(500);
const REDISCOVERY_WAITTIME_CONTACT_WITH_ULN: Duration = Duration::from_millis(1000);
const REDISCOVERY_WAITTIME_OTHER: Duration = Duration::from_millis(2000);
const REDISCOVERY_TIMEOUT_MAX: Duration = Duration::from_millis(250);

/// Configuration for use case [FailureHandling].
///
/// From the draft: The actual rediscovery messages are sent after different
/// randomly chosen waiting times from an interval [0.5tp, 1.5tp]. The mean
/// value t_p is set as follows: for invalidated ULNs 100ms, affected ID-wise
/// near contacts (in the deepest buckets) 500ms, for contacts affected by the
/// failure of a link to a ULN 1s and for all other affected contacts 2s.
///
/// Defaults:
///
/// - Overlay neighbors to notify via `UpdateRouteReq`: 3.
/// - Number of Bits grouped for calculation of closeness for overlay neighbors: 1 Bit.
/// - Intervall used for calculation of random timeout: `[0.5 t, 1.5 t]`,
///   t = 100ms (direct underlay neighbor)
///   t = 500ms (closest id-wise overlay neighbors),
///   t = 1s (contact affected by ULN failure),
///   t = 2s (all other contacts).
/// - Max retries for exponential backoff: 5.
#[derive(Debug)]
pub struct FailureHandlingConfig {
    /// Number of overlay neighbors to notify about a node failure.
    pub failure_notification_radius: NonZeroUsize,
    /// Number of bits used for determining closest overlay neighbors.
    pub grouping_bits: NonZeroU8,
    /// Number of contacts to try for rediscovery (normally same as bucket size)
    pub number_via_contacts: NonZeroUsize,
    /// Intervall used to generate random timeout durations based on distance to failing contact
    /// for exponential backoff.
    pub backoff_timeout_interval: RediscoveryTimeoutInterval,
    /// Number of times the rediscovery sends `FindNodeReq`s for a single failed contact.
    pub max_retry_rounds: NonZeroU32,
    /// Number of parallel rediscoveries
    pub rediscovery_parallelism: NonZeroUsize,
}

impl Default for FailureHandlingConfig {
    fn default() -> Self {
        Self {
            failure_notification_radius: NonZeroUsize::new(4).unwrap(),
            grouping_bits: NonZeroU8::new(1).unwrap(),
            number_via_contacts: NonZeroUsize::new(20).unwrap(),
            backoff_timeout_interval: RediscoveryTimeoutInterval::default(),
            max_retry_rounds: NonZeroU32::new(5).unwrap(),
            rediscovery_parallelism: NonZeroUsize::new(2).unwrap(),
        }
    }
}

/// Use case which handles every type of node failure or link failure.
///
/// ## Tasks
///
/// - Listens to [UnderlayNeighborUpdate]s and invalidates all affected contacts in the routing table.
#[derive(Debug)]
pub struct FailureHandling<C, const BUCKET_SIZE: usize> {
    _pd: PhantomData<C>,
    state: ReactiveUseCaseState,
    config: FailureHandlingConfig,
    rediscoveries: InflightReqMap,
    scheduled_rediscoveries: HashMap<TimerId, NodeId>,
}

impl<C, const BUCKET_SIZE: usize> FailureHandling<C, BUCKET_SIZE> {
    pub fn new(config: FailureHandlingConfig) -> Self {
        Self {
            _pd: Default::default(),
            state: ReactiveUseCaseState::Idle,
            config,
            rediscoveries: InflightReqMap::default(),
            scheduled_rediscoveries: HashMap::new(),
        }
    }
}

impl<C, const BUCKET_SIZE: usize> FailureHandling<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
    C::VicinityGraph: VicinityGraph,
{
    // returns the default rediscovery wait times according to different contacts types
    fn get_rediscovery_wait_time(&self, context: &C, contact: &Contact) -> Duration {
        match contact {
            contact if contact.is_uln() => REDISCOVERY_WAITTIME_ULN,
            contact if context.routing_table().is_close_contact(contact.id()) => {
                REDISCOVERY_WAITTIME_CLOSEST_NEIGHBORS
            }
            contact if contact.has_broken_first_hop() => REDISCOVERY_WAITTIME_CONTACT_WITH_ULN,
            _ => REDISCOVERY_WAITTIME_OTHER,
        }
    }

    // this should be called only from immediate error msgs, because the NotVia is using the current time
    fn invalidate_contacts_containing_link(&self, context: &C, failedlink: &Link) {
        for mut contact in context.routing_table_mut().iter_mut() {
            if contact.path().is_some() && contact.path().unwrap().contains_link(failedlink) {
                contact.set_invalid(NotViaStateList::from(NotViaState::new(
                    failedlink.clone(),
                    Timestamp::now(),
                )));

                log::trace!(target: "failure_handling", "Invalidated {} whose path contains {}", contact.id(), failedlink);
            }
        }
    }

    /// extracts the next [no_of_via_contacts] eligible via contacts from the Rediscovery state of the given [contact_id]
    /// returns None if contact is not in the expected Resdicovery state or the via contact list is empty or no usable via contact found
    fn extract_eligible_via_contacts(
        &mut self,
        context: &C,
        contact_id: &NodeId,
        no_of_via_contacts: usize,
    ) -> Option<Vec<NodeId>> {
        // extract the next via contacts
        // find next useful via contact from the list
        let mut next_via_contact_ids: Vec<NodeId> = Vec::new();
        let mut processed_via_contacts: usize = 0;
        // get contact for rediscovery state
        // if contact does not exist anymore then terminate Rediscovery
        // if contact state is still rediscovering get next via contacts to try
        if let ContactState::Rediscovering(rds) =
            context.routing_table().contact(contact_id)?.state()
        {
            // contact is still in rediscovering
            // get next via contacts from list that exists and is valid (note that this is then removed from the list in rds)
            let via_contact_list = rds.get_via_contact_list();
            let via_contact_list_iter = via_contact_list.iter().enumerate();
            for (idx, next_via_contact_id) in via_contact_list_iter {
                processed_via_contacts = idx + 1;
                if context
                    .routing_table()
                    .contains_with(next_via_contact_id, |c| c.is_valid())
                {
                    next_via_contact_ids.push(*next_via_contact_id);
                }
                if next_via_contact_ids.len() == no_of_via_contacts {
                    break;
                }
            }
        } else {
            // contact exists but is not in rediscovery state anymore, so simply ignore this timeout or message
            return None;
        }
        // now we need to delete the processed entries from the via contact list in a separate pass
        // due to mutable borrow
        if processed_via_contacts > 0
            && let ContactState::Rediscovering(rds_mut) = context
                .routing_table_mut()
                .contact_mut(contact_id)
                .unwrap() // is safe heredue to code above
                .state_mut()
        {
            rds_mut.drop_from_next_via_contact_id(processed_via_contacts);
        }

        if !next_via_contact_ids.is_empty() {
            return Some(next_via_contact_ids);
        }

        None
    }

    fn send_rediscovery_for_contact(&mut self, context: &C, contact_id: &NodeId) {
        // extracts up to (usuallly) two via contacts from the Rediscovery State
        let Some(closest_via_contacts) = self.extract_eligible_via_contacts(
            context,
            contact_id,
            self.config.rediscovery_parallelism.get(),
        ) else {
            // if we have no via contacts for whatever reason, we cannot send anything
            return;
        };

        // extract notviastate list from contact state
        let notviastate_list = match context.routing_table().contact(contact_id).unwrap().state() {
            ContactState::Rediscovering(rds) => rds.get_notviastate_list().clone(),
            _ => NotViaStateList::default(),
        };

        // send two rediscovery requests in parallel
        for via_contact_id in closest_via_contacts.iter() {
            // Add rediscovery state before sending find node in case of error
            let nonce = {
                let mut nonce = Nonce::random();
                // TODO maybe use measured RTT instead of fixed timeout
                let mut timer_id = context.runtime().register_timer(REDISCOVERY_TIMEOUT_MAX);

                // TODO this should use the state in the Rediscovering state
                while let Err(e) = self.rediscoveries.insert((*contact_id, timer_id, nonce)) {
                    match e {
                        InsertionError::DuplicateNonce => nonce = Nonce::random(),
                        InsertionError::DuplicateTimer => {
                            log::warn!(target: "failure_handling", "runtime emitted duplicate timer id {} [{:?}]", timer_id, self.rediscoveries);
                            timer_id = context.runtime().register_timer(REDISCOVERY_TIMEOUT_MAX);
                        }
                    }
                }

                log::trace!(target: "failure_handling", "Created rediscovery entry for {} [timeout: {:#?}, nonce: {:?}]", contact_id, REDISCOVERY_TIMEOUT_MAX, nonce);

                nonce
            };

            let rtable = context.routing_table();
            let via_contact = rtable
                .contact(via_contact_id)
                .expect("Contact should still be present in Routingtable");

            // send a findNodeReq for rediscovery to the via contact with target of the contact to be rediscovered
            let find_node_request = ReqRspMessage {
                common_header: CommonHeader::new(
                    ProtocolMessageKind::FindNodeReq,
                    *context.root_id(),
                    *via_contact_id,
                    Some(nonce.into()),
                    Some(From::from(*context.uln_table().state_seq_nr())),
                    context.uln_table().size(),
                ),
                data: FindNodeReqData {
                    exact: true,
                    neighborhood: NonZeroU64::new(BUCKET_SIZE as u64).unwrap(),
                    target: *contact_id,
                },
                not_via: From::from(notviastate_list.clone()),
                source_route: SourceRoute::new(
                    *context.root_id(),
                    via_contact.path().unwrap().clone(),
                ),
            };

            // send FindNodeReq message
            context.runtime().send_message(
                find_node_request,
                context.uln_table().deref(),
                context.root_id(),
            );

            log::trace!(target: "failure_handling", "Sent rediscovery find node for {} to {}", contact_id, via_contact.id());
        }
    }

    fn send_updates(&mut self, context: &C, contact: &Contact, notviastatelist: &NotViaStateList) {
        let closest_own_contacts = context
            .routing_table()
            .closest(
                context.root_id(),
                self.config.number_via_contacts.get(),
                self.config.grouping_bits,
            )
            .expect("invalid config");

        let mut updates = HashMap::new();
        updates.insert(contact.clone(), RouteUpdateActionType::Unreachable);
        // only use the self.config.failure_notification_radius.get() first contacts of
        for (_, closest_overlay_neighbor) in closest_own_contacts
            .iter()
            .take(self.config.failure_notification_radius.get())
        {
            let update_route_message = UpdateRouteReq {
                common_header: CommonHeader::new(
                    ProtocolMessageKind::UpdateRouteReq,
                    *context.root_id(),
                    *contact.id(),
                    None,
                    Some(From::from(*context.uln_table().state_seq_nr())),
                    context.uln_table().size(),
                ),
                not_via: From::from(notviastatelist.clone()),
                contact_actions: updates.clone(),
                source_route: SourceRoute::new(
                    *context.root_id(),
                    closest_overlay_neighbor.path().unwrap().clone(),
                ),
            };

            context.runtime().send_message(
                update_route_message,
                context.uln_table().deref(),
                context.root_id(),
            );
        }
    }

    // schedule rediscovery for contact
    fn schedule_rediscovery(&mut self, context: &C, contact: &Contact) {
        let timer_id = context
            .runtime()
            .register_timer(self.get_rediscovery_wait_time(context, contact));
        self.scheduled_rediscoveries.insert(timer_id, *contact.id());
    }

    /// Sends UpdateRouteReq to own closest overlay neighbors,
    /// initializes RediscoveryState and sends first FindNodeReq
    fn start_rediscovery(
        &mut self,
        context: &C,
        contact: Contact,
    ) -> Result<(), FailureHandlingError> {
        let ContactState::Invalid(notviastatelist) = contact.state() else {
            log::error!(target: "failure_handling:", "start_rediscovery called but contact state is not invalid");
            return Err(FailureHandlingError::WrongContactState);
        };

        // Send updates to id-wise neighbors in case a direct link to a ULN failed
        // Updates should be sent when direct links have failed and
        // otherwise after some time for now we sent Unreachable from here
        if contact.is_uln() {
            self.send_updates(context, &contact, notviastatelist);
        }

        let closest_via_contacts = context
            .routing_table()
            .closest(
                contact.id(),
                self.config.number_via_contacts.get(),
                self.config.grouping_bits,
            )
            .expect("invalid config");

        // contact state changes to Rediscovering
        context
            .routing_table_mut()
            .contact_mut(contact.id())
            .expect("contact should still be present in RT")
            .start_rediscovering(
                notviastatelist.clone(),
                closest_via_contacts.iter().map(|(_, x)| *x.id()).collect(),
            );

        self.schedule_rediscovery(context, &contact);

        Ok(())
    }

    /// This method is typically called either by an error message or a timeout for the corresponding request
    /// If the nodes exponential backoff has not reached max retries yet a new find node will be sent
    /// otherwise the contact gets removed.
    fn handle_rediscovery_failure(
        &mut self,
        context: &C,
        nonce: Option<Nonce>,
        timer_id: Option<TimerId>,
    ) -> Result<(), FailureHandlingError> {
        // we remove the entry from inflight requests
        let node_id = match (nonce, timer_id) {
            (None, None) => panic!(
                "internal error: handle_rediscovery must be called either with message nonce or timer timeout"
            ),
            (Some(nonce), None) | (Some(nonce), Some(_)) => {
                if self.rediscoveries.nonce_exists(&nonce) {
                    self.rediscoveries.remove_by_nonce(nonce).expect("internal error: handle_rediscovery: rediscoveries entry should be present").0
                } else {
                    // unknown noce for us, just return
                    return Ok(());
                }
            }
            (None, Some(timer_id)) => {
                if self.rediscoveries.timer_exists(&timer_id) {
                    self.rediscoveries.remove_by_timer(&timer_id).expect("internal error: handle_rediscovery: rediscoveries entry should be present").0
                } else {
                    // unknown timer for us, just return
                    return Ok(());
                }
            }
        };

        // it may be the case that the node has been removed from the routing table
        if !context.routing_table().contains(&node_id) {
            return Ok(());
        }

        let next_via_contact_id: Option<NodeId>;
        // find next useful via contact from the list
        // we only need to send one more out since this is the reaction to a previous rediscovery
        // extracts up to (usuallly) two via contacts from the Rediscovery State
        match self.extract_eligible_via_contacts(context, &node_id, 1) {
            Some(mut closest_via_contacts) => {
                next_via_contact_id = closest_via_contacts.pop();
            }
            None => {
                // nothing more to probe or contact in wrong state etc.
                // refill via contact list for another round
                let new_via_contact_list = context
                    .routing_table()
                    .closest(
                        &node_id,
                        self.config.number_via_contacts.get(),
                        self.config.grouping_bits,
                    )
                    .expect("invalid config")
                    .iter()
                    .map(|(_, x)| *x.id())
                    .collect();

                if let Some(mut contact) = context.routing_table_mut().contact_mut(&node_id)
                    && let ContactState::Rediscovering(rds) = contact.state_mut()
                {
                    // no usable contact found, try next round
                    if rds.retry_counter < self.config.max_retry_rounds.get() as u8 {
                        rds.retry_counter += 1;
                        // refill not via contacts
                        rds.set_via_contact_list(new_via_contact_list);
                    } else {
                        // retry counter reached maximum value
                        // set contact state to dead if there is no other rediscovery in flight
                        if !self.rediscoveries.exists(&node_id) {
                            log::debug!(target: "failure_handling", "Rediscovery of {node_id} failed. Removing from routing table");
                            contact.set_state(ContactState::Dead);
                        }
                        return Ok(());
                    }
                } else {
                    // contact not in state rediscovering, nothing to do here
                    return Ok(());
                }

                if let Some(mut next_via_contact_vec) =
                    self.extract_eligible_via_contacts(context, &node_id, 1)
                {
                    next_via_contact_id = next_via_contact_vec.pop();
                } else {
                    return Ok(());
                }
            }
        }

        // if there is no usable next via contact, we'll stop
        if next_via_contact_id.is_none() {
            return Ok(());
        }
        // now that we are sure that the id exists, reuse name as value
        let next_via_contact_id = next_via_contact_id.unwrap();
        // TODO different waiting times need to be implemented

        // Start new find node
        let timer_id = context.runtime().register_timer(REDISCOVERY_TIMEOUT_MAX);

        let nonce = {
            let mut nonce = Nonce::random();

            while let Err(e) = self.rediscoveries.insert((node_id, timer_id, nonce)) {
                match e {
                    InsertionError::DuplicateTimer => {
                        log::warn!(target: "failure_handling", "runtime emitted duplicate timer id {} [{:#?}]", timer_id, self.rediscoveries);
                        self.state = ReactiveUseCaseState::Error;
                        return Err(FailureHandlingError::DuplicateTimerId);
                    }
                    InsertionError::DuplicateNonce => nonce = Nonce::random(),
                }
            }

            nonce
        };

        // extract notviastate list from contact state
        let notviastate_list = match context.routing_table().contact(&node_id).unwrap().state() {
            ContactState::Rediscovering(rds) => rds.get_notviastate_list().clone(),
            _ => NotViaStateList::default(),
        };

        let find_node_request = ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::FindNodeReq,
                *context.root_id(),
                next_via_contact_id,
                Some(nonce.into()),
                Some(From::from(*context.uln_table().state_seq_nr())),
                context.uln_table().size(),
            ),
            data: FindNodeReqData {
                exact: true,
                neighborhood: NonZeroU64::new(BUCKET_SIZE as u64).unwrap(),
                target: node_id,
            },
            not_via: From::from(notviastate_list.clone()),
            source_route: SourceRoute::new(
                *context.root_id(),
                context
                    .routing_table()
                    .contact(&next_via_contact_id)
                    .as_ref()
                    .unwrap()
                    .path()
                    .expect("valid via contact must have valid path")
                    .clone(),
            ),
        };

        context.runtime().send_message(
            find_node_request,
            context.uln_table().deref(),
            context.root_id(),
        );

        let current_retries = match context.routing_table().contact(&node_id).unwrap().state() {
            ContactState::Rediscovering(rds) => rds.retry_counter,
            _ => {
                panic!("handle rediscovery failure: contact should be in Rediscovering state");
            }
        };
        // Just some logging, if logging is disabled this will be eliminated through dead code elimination
        log::trace!(target: "failure_handling", "Sent rediscovery find node for {} to {} [round {} of {}]", node_id, next_via_contact_id, current_retries, self.config.max_retry_rounds.get());

        Ok(())
    }

    /// Find all contacts whose paths start with underlay neighbors affected by the outage of an interface
    /// (there may be several ULNs behind a single interface)
    fn invalidate_affected_contacts(&self, context: &C, ulnid: UnderlayNeighborId) {
        let mut rt = context.routing_table_mut();

        let affected_underlay_neighbors = context
            .uln_table()
            .iter()
            .filter_map(|(id, via)| if via == &ulnid { Some(*id) } else { None })
            .collect::<HashSet<_>>();

        log::trace!(target: "failure_handling", "Underlay neighbors affected by interface {ulnid:?} down: {affected_underlay_neighbors:?}");

        // find contacts whose path contain affected underlay neighbors as first hop
        for mut contact in rt.iter_mut() {
            if let Some(active_path) = contact.path() {
                // contact possesses active path
                // invalidate if it starts with one of the affected ULNs
                let first_hop = *active_path.first();
                if affected_underlay_neighbors.contains(&first_hop) {
                    contact.set_invalid(NotViaStateList::from(NotViaState::new(
                        Link::new(*context.root_id(), first_hop),
                        Timestamp::now(),
                    )));

                    log::trace!(target: "failure_handling", "Invalidated contact {} as it starts with failed underlay neighbor {}", contact.id(), contact.path().unwrap().first());
                }
            }
        }

        // update vicinity graph
        let root_id = context.root_id();
        let mut vg_lock = context.vicinity_graph_mut();
        for neighbor in affected_underlay_neighbors.iter() {
            context.uln_table_mut().remove(neighbor);
            if vg_lock.remove_edge(root_id, neighbor) {
                tracing::debug!(
                    target: "failure_handling",
                    node = %neighbor,
                    %ulnid,
                    reason = "uln_down",
                    "removed node from vicinity graph"
                );
            }
        }
        for removed_node in vg_lock.retain_vicinity() {
            tracing::debug!(
                target: "failure_handling",
                node = %removed_node,
                reason = "outside_vicinity",
                "removed node from vicinity graph"
            );
        }
    }
}

impl<C, const BUCKET_SIZE: usize> EventHandler for FailureHandling<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
    C::VicinityGraph: VicinityGraph,
{
    type Context = C;
    type Error = FailureHandlingError;
    type Value = ();

    #[instrument(
        level = Level::TRACE,
        target = "failure_handling",
        "failure_handling",
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
            UseCaseEvent::Contact(contact_event) => {
                match *contact_event {
                    ContactEvent::Updated { new, old } if new.id() == old.id() => {
                        // contact was invalidated
                        if new.is_invalid() && old.is_valid() {
                            self.start_rediscovery(context, *new)?;
                        }
                    }
                    ContactEvent::Removed(contact) => {
                        // a contact may be removed by being replaced by a better contact while in rediscovery
                        self.rediscoveries.remove_by_id(contact.id());
                        log::trace!(target: "failure_handling", "Removed rediscovery for {} as it got removed from routing table", contact.id());
                    }
                    _ => {}
                }
            }
            UseCaseEvent::Message(ProtocolMessage::FindNodeRsp(rsp), _) => {
                if let Some((_, timer)) = self.rediscoveries.remove_by_nonce(rsp.msg_id().into()) {
                    log::trace!(target: "failure_handling", "Removed rediscovery for {} as it got a successful answer [timer: {:?}, nonce: {:?}]", rsp.source_route.source(), timer, rsp.msg_id());
                    log::debug!(target: "failure_handling", "Rediscovery of {} was successful!", rsp.source());
                }
            }
            UseCaseEvent::Message(ProtocolMessage::Error(rsp), _) => {
                // NotVia Data has to be added for failed link and contacts need to be invalidated
                if let ReqRspMessage {
                    data:
                        ErrorData::SegmentFailure {
                            ref failed_link, ..
                        },
                    ..
                } = rsp
                {
                    log::trace!(target: "failure_handling", "checking for affected contacts with failed link {failed_link:?}");
                    self.invalidate_contacts_containing_link(context, failed_link);
                }

                self.handle_rediscovery_failure(context, Some(rsp.msg_id().into()), None)?;
            }
            UseCaseEvent::Timer(id) => {
                if let Some(contact_id) = self.scheduled_rediscoveries.remove(&id) {
                    // start sending the first rediscovery messages in parallel
                    self.send_rediscovery_for_contact(context, &contact_id);
                } else {
                    // it is potentially a timeout
                    self.handle_rediscovery_failure(context, None, Some(id))?;
                }
            }
            UseCaseEvent::UnderlayUpdate(UnderlayNeighborUpdate::UnderlayNeighborDown(ulnid)) => {
                self.invalidate_affected_contacts(context, ulnid);
            }
            _ => {}
        }

        Ok(())
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase for FailureHandling<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
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

#[derive(Debug, Display, Error)]
pub enum FailureHandlingError {
    #[display("Runtime returned duplicate timer-id")]
    DuplicateTimerId,
    #[display("Wrong contact state")]
    WrongContactState,
}
