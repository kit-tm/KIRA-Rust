use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::marker::PhantomData;
use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};
use std::ops::Deref;

use crate::domain::{
    Contact, ContactState, Link, NodeId, NotVia, PNTable, RoutingTable, UnderlayNeighborId,
    UnderlayNeighborUpdate,
};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{
    ErrorData, FindNodeReqData, Nonce, ProtocolMessage, ReqRspMessage, RouteUpdate, UpdateRouteReq,
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
///     t = 1s (underlay neighbor), t = 2s (andernfalls).
/// - Max retries for exponential backoff: 5.
pub struct FailureHandlingConfig {
    /// Number of overlay neighbors to notify about a node failure.
    pub failure_notification_radius: NonZeroUsize,
    /// Number of bits used for determining closest overlay neighbors.
    pub grouping_bits: NonZeroUsize,
    /// Intervall used to generate random timeout durations based on distance to failing contact
    /// for exponential backoff.
    pub backoff_timeout_interval: RediscoveryTimeoutInterval,
    /// Number of times the rediscovery sends `FindNodeReq`s for a single failed contact.
    pub backoff_max_retries: NonZeroU32,
}

impl Default for FailureHandlingConfig {
    fn default() -> Self {
        Self {
            failure_notification_radius: NonZeroUsize::new(3).unwrap(),
            grouping_bits: NonZeroUsize::new(1).unwrap(),
            backoff_timeout_interval: RediscoveryTimeoutInterval::default(),
            backoff_max_retries: NonZeroU32::new(5).unwrap(),
        }
    }
}

/// Use case which handles every type of node failure or link failure.
///
/// ## Tasks
///
/// - Listens to [HardwareEvent]s and invalidates all affected contacts in the routing table.
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
    C::PhysicalNeighborTable: PNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    /// Remove all NotVia Data which is related to the contact
    fn remove_notvia_mentioning(&self, context: &C, contact_id: &NodeId) {
        context.not_via_mut().retain(|not_via| match not_via {
            NotVia::Link(link) => link.first() != contact_id && link.second() != contact_id,
        });
        log::trace!(target: "failure_handling", "Removed not via data mentioning {}", contact_id);
    }

    fn invalidate_containing_contacts(&self, context: &C, id: &NodeId) {
        for mut contact in context.routing_table_mut().iter_mut() {
            if contact.path().contains(id) {
                *contact.state_mut() = ContactState::Invalid;

                log::trace!(target: "failure_handling", "Invalidated {} whose path contains {}", contact.id(), id);
            }
        }
    }

    fn get_distance_to(&self, context: &C, id: &NodeId) -> Distance {
        if context.pn_table().contains(id) {
            return Distance::PhysicalNeighbor;
        }

        let mut closest = context
            .routing_table()
            .closest(
                context.root_id(),
                BUCKET_SIZE,
                self.config.grouping_bits.get(),
            )
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
        let closest = context
            .routing_table()
            .closest(
                context.root_id(),
                self.config.failure_notification_radius.get(),
                self.config.grouping_bits.get(),
            )
            .expect("invalid config");
        let mut updates = HashMap::new();
        updates.insert(contact.clone(), RouteUpdate::Updated);
        for (_, closest_overlay_neighbor) in closest {
            let update_route_message = UpdateRouteReq {
                source_state_seq_nr: *context.pn_table().state_seq_nr(),
                not_via: context.not_via().clone(),
                contact_actions: updates.clone(),
                source_route: SourceRoute::new(
                    *context.root_id(),
                    closest_overlay_neighbor.path().clone(),
                ),
            };

            context
                .runtime_mut()
                .send_message(update_route_message, context.pn_table().deref());
        }

        let closest = context
            .routing_table()
            .closest(contact.id(), 1, self.config.grouping_bits.get())
            .expect("invalid config");

        let closest_contact = match closest.first().cloned() {
            Some((_, closest)) => closest,
            None => {
                log::trace!(target: "failure_handling", "No closest contacts found. Assuming isolation.");
                return Ok(());
            }
        };

        // Add rediscovery state before sending find node in case of error
        let start_duration = self
            .config
            .backoff_timeout_interval
            .gen(self.get_distance_to(context, contact.id()));
        let mut exponential_backoff = ExponentialBackoff::with_default_base(
            self.config.backoff_max_retries.get(),
            start_duration,
        );
        let timer_duration = exponential_backoff
            .next()
            .expect("configuration was invalid");

        let nonce = {
            let mut nonce = Nonce::random();
            let mut timer_id = context.runtime_mut().register_timer(timer_duration);

            while let Err(e) = self.rediscoveries.insert(
                (*contact.id(), timer_id, nonce.clone()),
                exponential_backoff.clone(),
            ) {
                match e {
                    InsertionError::DuplicateNonce => nonce = Nonce::random(),
                    InsertionError::DuplicateTimer => {
                        log::warn!(target: "failure_handling", "runtime emitted duplicate timer id {} [{:?}]", timer_id, self.rediscoveries);
                        timer_id = context.runtime_mut().register_timer(timer_duration)
                    }
                }
            }

            log::trace!(target: "failure_handling", "Created rediscovery entry for {} [backoff: {}, nonce: {:?}]", contact.id(), exponential_backoff, nonce);

            nonce
        };

        let find_node_request = ReqRspMessage {
            nonce,
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
            data: FindNodeReqData {
                exact: true,
                neighborhood: NonZeroU64::new(BUCKET_SIZE as u64).unwrap(),
                target: *contact.id(),
            },
            not_via: context.not_via().clone(),
            source_route: SourceRoute::new(*context.root_id(), closest_contact.path().clone()),
        };

        context
            .runtime_mut()
            .send_message(find_node_request, context.pn_table().deref());

        log::trace!(target: "failure_handling", "Sent rediscovery find node for {} to {} [retry {} of {}]", contact.id(), closest_contact.id(), exponential_backoff.current_retries, exponential_backoff.max_retries);

        Ok(())
    }

    /// If the nodes exponential backoff has not reached max retries yet a new find node will be sent
    /// otherwise the contact gets removed.
    fn handle_rediscovery_failure(
        &mut self,
        context: &C,
        node_id: &NodeId,
    ) -> Result<(), FailureHandlingError> {
        let closest = context
            .routing_table()
            .closest(node_id, 1, self.config.grouping_bits.get())
            .expect("invalid config");

        let closest_contact = match closest.first().cloned() {
            Some((_, closest)) => closest,
            None => {
                log::trace!(target: "failure_handling", "No closest contacts found. Assuming isolation.");
                context.routing_table_mut().remove(node_id);
                context.pn_table_mut().remove(node_id);
                self.rediscoveries.remove(node_id);
                return Ok(());
            }
        };

        let backoff = self.rediscoveries.get_mut(node_id);
        assert!(
            backoff.is_some(),
            "handle_rediscovery_failure should only be called for nodes with running rediscovery"
        );
        let next_duration = backoff.unwrap().next();
        if next_duration.is_none() {
            log::trace!(target: "failure_handling", "Rediscovery of {} failed. Removing from routing table", node_id);
            context.pn_table_mut().remove(node_id);
            context.routing_table_mut().remove(node_id);
            self.remove_notvia_mentioning(context, node_id);
            self.rediscoveries.remove(node_id);
            return Ok(());
        }
        let next_duration = next_duration.unwrap();

        // Start new find node
        let timer_id = context.runtime_mut().register_timer(next_duration);

        let response = self.rediscoveries.replace_timer_for(*node_id, timer_id);
        if response.is_err() {
            log::warn!(target: "failure_handling", "runtime emitted duplicate timer id {} [{:#?}]", timer_id, self.rediscoveries);
            self.state = ReactiveUseCaseState::Error;
            return Err(FailureHandlingError::DuplicateTimerId);
        }

        let nonce = {
            let mut nonce = Nonce::random();

            while let Err(e) = self
                .rediscoveries
                .add_nonce_for_node(*node_id, nonce.clone())
            {
                match e {
                    AddNonceError::UnknownNode => {
                        panic!("handle_rediscovery_failure was called with unknown node")
                    }
                    AddNonceError::DuplicateNonce => nonce = Nonce::random(),
                }
            }

            nonce
        };
        let find_node_request = ReqRspMessage {
            nonce,
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
            data: FindNodeReqData {
                exact: true,
                neighborhood: NonZeroU64::new(BUCKET_SIZE as u64).unwrap(),
                target: *node_id,
            },
            not_via: context.not_via().clone(),
            source_route: SourceRoute::new(*context.root_id(), closest_contact.path().clone()),
        };

        context
            .runtime_mut()
            .send_message(find_node_request, context.pn_table().deref());

        // Just some logging, if logging is disabled this will be eliminated through dead code elimination
        if let Some(exponential_backoff) = self.rediscoveries.get(node_id) {
            log::trace!(target: "failure_handling", "Sent rediscovery find node for {} to {} [retry {} of {}]", node_id, closest_contact.id(), exponential_backoff.current_retries, exponential_backoff.max_retries);
        }

        Ok(())
    }

    /// Find all contacts whose paths start with neighbors affected by the outage.
    fn invalidate_affected_contacts(&self, context: &C, ulnid: UnderlayNeighborId) {
        let mut rt = context.routing_table_mut();
        let mut not_via = context.not_via_mut();

        let affected_neighbors = context
            .pn_table()
            .iter()
            .filter_map(|(id, via)| if via == &ulnid { Some(*id) } else { None })
            .collect::<HashSet<_>>();

        log::trace!(target: "failure_handling", "These neighbors are affected by interfaces {:?} down: {:?}", ulnid, affected_neighbors);

        not_via.extend(
            affected_neighbors
                .iter()
                .cloned()
                .map(|id| NotVia::Link(Link::new(*context.root_id(), id))),
        );

        for mut contact in rt.iter_mut() {
            if affected_neighbors.contains(contact.path().first()) {
                *contact.state_mut() = ContactState::Invalid;

                log::trace!(target: "failure_handling", "Invalidated contact {} as it starts with an invalid neighbor {}", contact.id(), contact.path().first());
            }
        }

        for neighbor in affected_neighbors.iter() {
            context.pn_table_mut().remove(neighbor);
        }
    }
}

impl<C, const BUCKET_SIZE: usize> EventHandler for FailureHandling<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::PhysicalNeighborTable: PNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    type Context = C;
    type Error = FailureHandlingError;
    type Value = ();

    fn handle_event(
        &mut self,
        context: &C,
        event: UseCaseEvent,
    ) -> Result<Self::Value, Self::Error> {
        match event {
            UseCaseEvent::Contact(ContactEvent::Updated { new, old }) => {
                // contact was invalidated
                if new.state() == &ContactState::Invalid && old.state() != &ContactState::Invalid {
                    self.invalidate_containing_contacts(context, new.id());
                    self.start_rediscovery(context, new)?;
                } else if new.state() == &ContactState::Valid && old.state() != &ContactState::Valid
                {
                    self.remove_notvia_mentioning(context, new.id());
                }
            }
            UseCaseEvent::Contact(ContactEvent::Removed(contact)) => {
                if let Some((backoff, removed_timers, removed_nonces)) =
                    self.rediscoveries.remove(contact.id())
                {
                    log::trace!(target: "failure_handling", "Removed rediscovery for {} as it got removed from routing table [backoff_state: {}, timers: {:?}, nonces: {:?}]", contact.id(), backoff, removed_timers, removed_nonces);
                }
                self.remove_notvia_mentioning(context, contact.id());
            }
            UseCaseEvent::Message(ProtocolMessage::FindNodeRsp(rsp), _) => {
                if let Some((backoff, timers, nonces)) =
                    self.rediscoveries.remove_by_nonce(&rsp.nonce)
                {
                    log::trace!(target: "failure_handling", "Removed rediscovery for {} as it got a successful answer [backoff_state: {}, timers: {:?}, nonce: {:?}]", rsp.source_route.source(), backoff, timers, nonces);
                    log::debug!(target: "failure_handling", "Rediscovery of {} was successful!", rsp.source());
                }
            }
            UseCaseEvent::Message(ProtocolMessage::Error(rsp), _) => {
                if let Some(node) = self.rediscoveries.get_node_for_nonce(&rsp.nonce).cloned() {
                    self.rediscoveries.remove_nonce(&rsp.nonce);
                    self.handle_rediscovery_failure(context, &node)?;
                }
                // Anyways NotVia Data has to be added for failed link
                if let ReqRspMessage {
                    data: ErrorData::SegmentFailure { failed_link, .. },
                    ..
                } = rsp
                {
                    context
                        .not_via_mut()
                        .insert(NotVia::Link(failed_link.clone()));
                    log::trace!(target: "failure_handling", "added failed link {:?} to NotVia data", failed_link);
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
    C::PhysicalNeighborTable: PNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    type State = ReactiveUseCaseState;

    fn start(&mut self, _context: &C) -> Result<(), Self::Error> {
        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum FailureHandlingError {
    DuplicateTimerId,
}

impl Display for FailureHandlingError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateTimerId => write!(f, "Runtime returned duplicate timer-id"),
        }
    }
}

impl Error for FailureHandlingError {}
