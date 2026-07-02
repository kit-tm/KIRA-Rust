use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::num::{NonZeroU8, NonZeroU32, NonZeroU64, NonZeroUsize};
use std::ops::Deref;
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
    ContactEvent, EventHandler, ReactiveUseCaseState, UseCase, UseCaseContext, UseCaseEvent,
    UseCaseRuntime,
};
use crate::utils::errors::{AddNonceError, InsertionError};
use crate::utils::rediscovery_timeout_interval::{Distance, RediscoveryTimeoutInterval};
use crate::utils::{BackoffMap, ExponentialBackoff};

/// Configuration for use case [FailureHandling].
///
/// Defaults:
///
/// - Overlay neighbors to notify via `UpdateRouteReq`: 3.
/// - Number of Bits grouped for calculation of closeness for overlay neighbors: 1 Bit.
/// - Intervall used for calculation of random timeout: `[0.5 t, 1.5 t]`, t = 500ms (overlay neighbor),
///   t = 1s (underlay neighbor), t = 2s (otherwise).
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
    pub backoff_max_retries: NonZeroU32,
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
            backoff_max_retries: NonZeroU32::new(5).unwrap(),
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
    rediscoveries: BackoffMap,
}

impl<C, const BUCKET_SIZE: usize> FailureHandling<C, BUCKET_SIZE> {
    pub fn new(config: FailureHandlingConfig) -> Self {
        Self {
            _pd: Default::default(),
            state: ReactiveUseCaseState::Idle,
            config,
            rediscoveries: BackoffMap::default(),
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

    fn get_distance_to(&self, context: &C, id: &NodeId) -> Distance {
        if context.uln_table().contains(id) {
            return Distance::UnderlayNeighbor;
        }

        let mut closest = context
            .routing_table()
            .closest(context.root_id(), BUCKET_SIZE, self.config.grouping_bits)
            .expect("grouping should be valid");
        if closest.drain(..).any(|(_, contact)| contact.id() == id) {
            return Distance::OverlayNeighbor;
        }

        Distance::Others
    }

    /// Sends UpdateRouteReq to own closest overlay neighbors, sends first FindNodeReq and
    /// initializes RediscoveryState.
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
        // TODO Updates should be sent by a separate method and
        // when direct links have failed and otherwise after some time
        // for now we sent Unreachable from here
        if contact.is_uln() {
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

                context
                    .runtime()
                    .send_message(update_route_message, context.uln_table().deref());
            }
        }

        let closest_via_contacts = context
            .routing_table()
            .closest(
                contact.id(),
                self.config.number_via_contacts.get(),
                self.config.grouping_bits,
            )
            .expect("invalid config");

        // send two rediscovery requests in parallel
        for (_, closest_contact) in closest_via_contacts
            .iter()
            .take(self.config.rediscovery_parallelism.get())
        {
            // Add rediscovery state before sending find node in case of error
            let start_duration = self
                .config
                .backoff_timeout_interval
                .next_duration(self.get_distance_to(context, contact.id()));
            let mut exponential_backoff = ExponentialBackoff::with_default_base(
                self.config.backoff_max_retries.get(),
                start_duration,
            );
            let timer_duration = exponential_backoff
                .next()
                .expect("configuration was invalid");

            let nonce = {
                let mut nonce = Nonce::random();
                let mut timer_id = context.runtime().register_timer(timer_duration);

                // TODO this should use the state in the Rediscovering state
                while let Err(e) = self.rediscoveries.insert(
                    (*contact.id(), timer_id, nonce),
                    exponential_backoff.clone(),
                ) {
                    match e {
                        InsertionError::DuplicateNonce => nonce = Nonce::random(),
                        InsertionError::DuplicateTimer => {
                            log::warn!(target: "failure_handling", "runtime emitted duplicate timer id {} [{:?}]", timer_id, self.rediscoveries);
                            timer_id = context.runtime().register_timer(timer_duration)
                        }
                    }
                }

                log::trace!(target: "failure_handling", "Created rediscovery entry for {} [backoff: {}, nonce: {:?}]", contact.id(), exponential_backoff, nonce);

                nonce
            };

            // send a findNodeReq for rediscovery
            let find_node_request = ReqRspMessage {
                common_header: CommonHeader::new(
                    ProtocolMessageKind::FindNodeReq,
                    *context.root_id(),
                    *closest_contact.id(),
                    Some(nonce.into()),
                    Some(From::from(*context.uln_table().state_seq_nr())),
                    context.uln_table().size(),
                ),
                data: FindNodeReqData {
                    exact: true,
                    neighborhood: NonZeroU64::new(BUCKET_SIZE as u64).unwrap(),
                    target: *contact.id(),
                },
                not_via: From::from(notviastatelist.clone()),
                source_route: SourceRoute::new(
                    *context.root_id(),
                    closest_contact.path().unwrap().clone(),
                ),
            };

            // send FindNodeReq message
            context
                .runtime()
                .send_message(find_node_request, context.uln_table().deref());

            log::trace!(target: "failure_handling", "Sent rediscovery find node for {} to {} [retry {} of {}]", contact.id(), closest_contact.id(), exponential_backoff.current_retries, exponential_backoff.max_retries);
        }

        // contact state changes to Rediscovering
        context
            .routing_table_mut()
            .contact_mut(contact.id())
            .expect("contact should still be present in RT")
            .start_rediscovering(
                notviastatelist.clone(),
                closest_via_contacts
                    .iter()
                    .skip(self.config.rediscovery_parallelism.get())
                    .map(|(_, x)| *x.id())
                    .collect(),
            );

        Ok(())
    }

    /// This method is typically called either by an error message or a timeout for the corresponding request
    /// If the nodes exponential backoff has not reached max retries yet a new find node will be sent
    /// otherwise the contact gets removed.
    fn handle_rediscovery_failure(
        &mut self,
        context: &C,
        node_id: &NodeId,
    ) -> Result<(), FailureHandlingError> {
        // find next useful via contact from the list
        let mut next_via_contact_id = None;
        let mut checked_via_contacts: usize = 0;
        if let Some(contact) = context.routing_table().contact(node_id) {
            if let ContactState::Rediscovering(rds) = contact.state() {
                // get next via contact from list that exists and is valid
                if let Some(result) =
                    rds.get_via_contact_list()
                        .iter()
                        .enumerate()
                        .find(|(_idx, nid)| {
                            context.routing_table().contains_with(nid, |c| c.is_valid())
                        })
                {
                    next_via_contact_id = Some(*result.1);
                    checked_via_contacts = result.0 + 1; // need to add one to index
                } else {
                    // nothing found, need to clear the whole list then
                    checked_via_contacts = rds.get_via_contact_list().len();
                }
            }
        } else {
            // if contact does not exist anymore or is not in Rediscovering state, terminate Rediscovery
            return Ok(());
        };

        // we need to remove any tried not via contacts from the rds state first
        // a second pass is required due to mutable borrow
        if checked_via_contacts > 0
            && let ContactState::Rediscovering(rds) = context
                .routing_table_mut()
                .contact_mut(node_id)
                .unwrap()
                .state_mut()
        {
            // now pop as many next via contacts that have been skipped
            while checked_via_contacts > 0 && rds.get_next_via_contact_id().is_some() {
                checked_via_contacts -= 1;
            }
            assert!(checked_via_contacts == 0); // must be true, otherwise list is too short
        }

        // if there is no usable next via contact, we'll stop
        if next_via_contact_id.is_none() {
            return Ok(());
        }
        // now that we are sure that the id exists, reuse name as value
        let next_via_contact_id = next_via_contact_id.unwrap();
        // TODO the exponential backoff needs to be checked
        // TODO different waiting times need to be implemented
        let backoff = self.rediscoveries.get_mut(node_id);
        assert!(
            backoff.is_some(),
            "handle_rediscovery_failure should only be called for nodes with running rediscovery"
        );
        let next_duration = backoff.unwrap().next();
        if next_duration.is_none() {
            log::debug!(target: "failure_handling", "Rediscovery of {node_id} failed. Removing from routing table");
            context.uln_table_mut().remove(node_id);
            context.routing_table_mut().remove(node_id);
            self.rediscoveries.remove(node_id);
            return Ok(());
        }
        let next_duration = next_duration.unwrap();

        // Start new find node
        let timer_id = context.runtime().register_timer(next_duration);

        let response = self.rediscoveries.replace_timer_for(*node_id, timer_id);
        if response.is_err() {
            log::warn!(target: "failure_handling", "runtime emitted duplicate timer id {} [{:#?}]", timer_id, self.rediscoveries);
            self.state = ReactiveUseCaseState::Error;
            return Err(FailureHandlingError::DuplicateTimerId);
        }

        let nonce = {
            let mut nonce = Nonce::random();

            while let Err(e) = self.rediscoveries.add_nonce_for_node(*node_id, nonce) {
                match e {
                    AddNonceError::UnknownNode => {
                        panic!("handle_rediscovery_failure was called with unknown node")
                    }
                    AddNonceError::DuplicateNonce => nonce = Nonce::random(),
                }
            }

            nonce
        };

        // extract notviastate list from contact state
        let notviastate_list = match context.routing_table().contact(node_id).unwrap().state() {
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
                target: *node_id,
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

        context
            .runtime()
            .send_message(find_node_request, context.uln_table().deref());

        // Just some logging, if logging is disabled this will be eliminated through dead code elimination
        if let Some(exponential_backoff) = self.rediscoveries.get(node_id) {
            log::trace!(target: "failure_handling", "Sent rediscovery find node for {} to {} [retry {} of {}]", node_id, next_via_contact_id, exponential_backoff.current_retries, exponential_backoff.max_retries);
        }

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
                        if let Some((backoff, removed_timers, removed_nonces)) =
                            self.rediscoveries.remove(contact.id())
                        {
                            log::trace!(target: "failure_handling", "Removed rediscovery for {} as it got removed from routing table [backoff_state: {}, timers: {:?}, nonces: {:?}]", contact.id(), backoff, removed_timers, removed_nonces);
                        }
                    }
                    _ => {}
                }
            }
            UseCaseEvent::Message(ProtocolMessage::FindNodeRsp(rsp), _) => {
                if let Some((backoff, timers, nonces)) =
                    self.rediscoveries.remove_by_nonce(rsp.msg_id().into())
                {
                    log::trace!(target: "failure_handling", "Removed rediscovery for {} as it got a successful answer [backoff_state: {}, timers: {:?}, nonce: {:?}]", rsp.source_route.source(), backoff, timers, nonces);
                    log::debug!(target: "failure_handling", "Rediscovery of {} was successful!", rsp.source());
                }
            }
            UseCaseEvent::Message(ProtocolMessage::Error(rsp), _) => {
                if let Some(node) = self
                    .rediscoveries
                    .get_node_for_nonce(rsp.msg_id().into())
                    .cloned()
                {
                    self.rediscoveries.remove_nonce(rsp.msg_id().into());
                    self.handle_rediscovery_failure(context, &node)?;
                }
                // Anyways NotVia Data has to be added for failed link
                if let ReqRspMessage {
                    data: ErrorData::SegmentFailure { failed_link, .. },
                    ..
                } = rsp
                {
                    log::trace!(target: "failure_handling", "checking for affected contacts with failed link {failed_link:?}");
                    self.invalidate_contacts_containing_link(context, &failed_link);
                }
            }
            UseCaseEvent::Timer(id) => {
                if let Some(node) = self.rediscoveries.get_node_for_timer(&id).cloned() {
                    self.handle_rediscovery_failure(context, &node)?;
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
