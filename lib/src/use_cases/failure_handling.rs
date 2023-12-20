use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::marker::PhantomData;
use std::num::{NonZeroU32, NonZeroU64, NonZeroUsize};

use crate::context::UseCaseContext;
use crate::domain::{Contact, ContactState, Link, NetworkInterface, NodeId, NotVia, RoutingTable};
use crate::hardware_events::HardwareEvent;
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{
    ErrorData, FindNodeReqData, Nonce, ProtocolMessage, ProtocolMessageSender, ReqRspMessage,
    RouteUpdate, UpdateRouteReq,
};
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{ContactEvent, EventHandler, ReactiveUseCaseState, UseCase, UseCaseEvent};
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
///     t = 1s (physical neighbor), t = 2s (andernfalls).
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

impl<C, const BUCKET_SIZE: usize> FailureHandling<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::MessageSender: ProtocolMessageSender,
    C::Runtime: UseCaseRuntime,
{
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
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::MessageSender: ProtocolMessageSender,
    C::Runtime: UseCaseRuntime,
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
                    context.root_id().clone(),
                    closest_overlay_neighbor.path().clone(),
                ),
            };

            if let Err(e) = context
                .message_sender_mut()
                .send_message(update_route_message)
            {
                log::error!(target: "failure_handling", "Failed to send update route request to {}: {}", contact.id(), e);
                return Err(FailureHandlingError::MessageSentFailed);
            }
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
            let mut timer_id = context.runtime().register_timer(timer_duration);

            while let Err(e) = self.rediscoveries.insert(
                (contact.id().clone(), timer_id, nonce.clone()),
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

        let find_node_request = ReqRspMessage {
            nonce,
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
            data: FindNodeReqData {
                exact: true,
                neighborhood: NonZeroU64::new(BUCKET_SIZE as u64).unwrap(),
                target: contact.id().clone(),
                origin_degree: context.pn_table().size(), // Add here, because root id is always in two last buckets
                origin_neighbor_id_sum: context.pn_table().neighbor_sum(),
            },
            not_via: context.not_via().clone(),
            source_route: SourceRoute::new(
                context.root_id().clone(),
                closest_contact.path().clone(),
            ),
        };

        if let Err(e) = context.message_sender_mut().send_message(find_node_request) {
            log::error!(target: "failure_handling", "Failed to send find node to {}: {}", closest_contact.id(), e);
        }

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
                log::debug!(target: "failure_handling", "No closest contacts found. Assuming isolation.");
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
            log::debug!(target: "failure_handling", "Rediscovery of {} failed. Removing from routing table", node_id);
            context.pn_table_mut().remove(node_id);
            context.routing_table_mut().remove(node_id);
            self.remove_notvia_mentioning(context, node_id);
            self.rediscoveries.remove(node_id);
            return Ok(());
        }
        let next_duration = next_duration.unwrap();

        // Start new find node
        let timer_id = context.runtime().register_timer(next_duration);

        let response = self
            .rediscoveries
            .replace_timer_for(node_id.clone(), timer_id);
        if response.is_err() {
            log::warn!(target: "failure_handling", "runtime emitted duplicate timer id {} [{:#?}]", timer_id, self.rediscoveries);
            self.state = ReactiveUseCaseState::Error;
            return Err(FailureHandlingError::DuplicateTimerId);
        }

        let nonce = {
            let mut nonce = Nonce::random();

            while let Err(e) = self
                .rediscoveries
                .add_nonce_for_node(node_id.clone(), nonce.clone())
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

        let (origin_degree, origin_neighbor_id_sum) = match context.routing_table().is_in_last_two_buckets(node_id) {
            true => (context.pn_table().size(), context.pn_table().neighbor_sum()),
            false => (None, None)
        };
        let find_node_request = ReqRspMessage {
            nonce,
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
            data: FindNodeReqData {
                exact: true,
                neighborhood: NonZeroU64::new(BUCKET_SIZE as u64).unwrap(),
                target: node_id.clone(),
                origin_degree,
                origin_neighbor_id_sum
            },
            not_via: context.not_via().clone(),
            source_route: SourceRoute::new(
                context.root_id().clone(),
                closest_contact.path().clone(),
            ),
        };

        if let Err(e) = context.message_sender_mut().send_message(find_node_request) {
            log::error!(target: "failure_handling", "Failed to send find node to {}: {}", closest_contact.id(), e);
        }

        // Just some logging, if logging is disabled this will be eliminated through dead code elimination
        if let Some(exponential_backoff) = self.rediscoveries.get(node_id) {
            log::trace!(target: "failure_handling", "Sent rediscovery find node for {} to {} [retry {} of {}]", node_id, closest_contact.id(), exponential_backoff.current_retries, exponential_backoff.max_retries);
        }

        Ok(())
    }

    /// Find all contacts whose paths start with neighbors affected by the outage.
    fn invalidate_affected_contacts(&self, context: &C, interfaces: HashSet<NetworkInterface>) {
        let mut rt = context.routing_table_mut();
        let mut not_via = context.not_via_mut();

        let affected_neighbors = context
            .pn_table()
            .iter()
            .filter_map(|(id, iface)| {
                if interfaces.contains(iface) {
                    Some(id.clone())
                } else {
                    None
                }
            })
            .collect::<HashSet<_>>();

        log::trace!(target: "failure_handling", "These neighbors are affected by interfaces {:?} down: {:?}", interfaces, affected_neighbors);

        not_via.extend(
            affected_neighbors
                .iter()
                .cloned()
                .map(|id| NotVia::Link(Link::new(context.root_id().clone(), id))),
        );

        for mut contact in rt.iter_mut() {
            if affected_neighbors.contains(contact.path().first()) {
                *contact.state_mut() = ContactState::Invalid;

                log::trace!(target: "failure_handling", "Invalidated contact {} as it starts with an invalid neighbor {}", contact.id(), contact.path().first());
            }
        }
    }
}

impl<C, const BUCKET_SIZE: usize> EventHandler for FailureHandling<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::MessageSender: ProtocolMessageSender,
    C::Runtime: UseCaseRuntime,
{
    type Context = C;
    type Error = FailureHandlingError;
    type Value = ();

    fn handle_event(
        &mut self,
        context: &Self::Context,
        event: UseCaseEvent,
    ) -> Result<Self::Value, Self::Error> {
        match event {
            UseCaseEvent::Contact(ContactEvent::Updated { new, old }) => {
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
                if let Some((backoff, timers, nonces)) = self
                    .rediscoveries
                    .remove_by_nonce(&rsp.nonce)
                    .map(|(state, timers, nonces)| (state, timers, nonces))
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
            UseCaseEvent::Hardware(HardwareEvent::InterfacesDown(interfaces)) => {
                self.invalidate_affected_contacts(context, interfaces);
            }
            _ => {}
        }

        Ok(())
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase for FailureHandling<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::MessageSender: ProtocolMessageSender,
    C::Runtime: UseCaseRuntime,
{
    type State = ReactiveUseCaseState;

    fn start(&mut self, _context: &Self::Context) -> Result<(), Self::Error> {
        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum FailureHandlingError {
    DuplicateTimerId,
    MessageSentFailed,
}

impl Display for FailureHandlingError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateTimerId => write!(f, "Runtime returned duplicate timer-id"),
            Self::MessageSentFailed => write!(f, "Failed to send protocol message"),
        }
    }
}

impl Error for FailureHandlingError {}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::num::{NonZeroU32, NonZeroUsize};
    use std::time::Duration;

    use crate::context::{ContextConfig, SyncContext, UseCaseContext};
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::domain::{
        Contact, ContactState, InsertionStrategyResult, Link, NetworkInterface, NodeId, PNTable,
        Path, RoutingTable, StateSeqNr, TestInsertionStrategy,
    };
    use crate::forwarding::in_memory_tables::InMemoryFwdTables;
    use crate::hardware_events::HardwareEvent;
    use crate::messaging::source_route::SourceRoute;
    use crate::messaging::{
        AsyncProtocolMessageReceiver, ErrorData, InMemoryMessageChannel, ProtocolMessage,
        ReqRspMessage,
    };
    use crate::runtime::ImmediateRuntime;
    use crate::use_cases::failure_handling::{FailureHandling, FailureHandlingConfig};
    use crate::use_cases::{ContactEvent, EventHandler, UseCase, UseCaseEvent};
    use crate::utils::rediscovery_timeout_interval::{DistanceMap, RediscoveryTimeoutInterval};

    #[tokio::test]
    async fn update_route_req_is_sent_for_invalidated_contact() {
        crate::tests::init();

        let interface = NetworkInterface::new("test");

        let root_id = NodeId::with_lsb(1);
        let neighbor_id = NodeId::with_lsb(2);
        let failed_id = NodeId::with_lsb(3);

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(30);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, mut hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(2));
        let failed_contact_before = Contact::new(
            Path::from([neighbor_id.clone(), failed_id.clone()]),
            StateSeqNr::from(3),
        );
        let mut failed_contact_after = Contact::new(
            Path::from([neighbor_id.clone(), failed_id.clone()]),
            StateSeqNr::from(3),
        );
        *failed_contact_after.state_mut() = ContactState::Invalid;

        let mut routing_table = SingleBucketRT::<20>::new(root_id.clone());
        assert!(routing_table.insert(neighbor.clone()).is_ok());
        assert!(routing_table.insert(failed_contact_before.clone()).is_ok());

        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());

        let sync_context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table,
            insertion_strategy: TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            message_sender: hub_sender,
            runtime: runtime.clone(),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let config = FailureHandlingConfig {
            failure_notification_radius: NonZeroUsize::new(1).unwrap(),
            grouping_bits: NonZeroUsize::new(1).unwrap(),
            backoff_timeout_interval: Default::default(),
            backoff_max_retries: NonZeroU32::new(2).unwrap(),
        };
        let mut use_case = FailureHandling::new(config);
        assert!(
            use_case.start(&sync_context).is_ok(),
            "failed to start use case"
        );

        let event = UseCaseEvent::Contact(ContactEvent::Updated {
            old: failed_contact_before,
            new: failed_contact_after,
        });
        let result = use_case.handle_event(&sync_context, event);
        assert!(
            result.is_ok(),
            "Failed to handle contact invalidation event"
        );

        // First UpdateRoute is sent
        let update_route = hub_receiver.try_recv().await;
        assert!(
            matches!(
                update_route,
                Ok(Some((ProtocolMessage::UpdateRouteReq(_), _)))
            ),
            "First message is not a UpdateRouteReq: {:?}",
            update_route
        );
    }

    #[tokio::test]
    async fn error_responses_yield_new_find_node_until_max_retries() {
        crate::tests::init();

        let interface = NetworkInterface::new("test");

        let root_id = NodeId::with_lsb(1);
        let neighbor_id = NodeId::with_lsb(2);
        let failed_id = NodeId::with_lsb(3);

        let failed_link = Link::new(neighbor_id.clone(), failed_id.clone());

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(30);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, mut hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(2));
        let failed_contact_before = Contact::new(
            Path::from([neighbor_id.clone(), failed_id.clone()]),
            StateSeqNr::from(3),
        );
        let mut failed_contact_after = Contact::new(
            Path::from([neighbor_id.clone(), failed_id.clone()]),
            StateSeqNr::from(3),
        );
        *failed_contact_after.state_mut() = ContactState::Invalid;

        let mut routing_table = SingleBucketRT::<20>::new(root_id.clone());
        assert!(routing_table.insert(neighbor.clone()).is_ok());
        assert!(routing_table.insert(failed_contact_before.clone()).is_ok());

        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());

        let sync_context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table,
            insertion_strategy: TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            message_sender: hub_sender,
            runtime: runtime.clone(),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let config = FailureHandlingConfig {
            failure_notification_radius: NonZeroUsize::new(1).unwrap(),
            grouping_bits: NonZeroUsize::new(1).unwrap(),
            backoff_timeout_interval: Default::default(),
            backoff_max_retries: NonZeroU32::new(2).unwrap(),
        };
        let mut use_case = FailureHandling::new(config);
        assert!(
            use_case.start(&sync_context).is_ok(),
            "failed to start use case"
        );

        let event = UseCaseEvent::Contact(ContactEvent::Updated {
            old: failed_contact_before,
            new: failed_contact_after,
        });
        let result = use_case.handle_event(&sync_context, event);
        assert!(
            result.is_ok(),
            "Failed to handle contact invalidation event"
        );

        // First UpdateRoute is sent
        let update_route = hub_receiver.try_recv().await;
        assert!(
            matches!(
                update_route,
                Ok(Some((ProtocolMessage::UpdateRouteReq(_), _)))
            ),
            "First message is not a UpdateRouteReq: {:?}",
            update_route
        );

        // Then a FindNodeReq is sent
        let find_node = hub_receiver.try_recv().await;
        assert!(
            find_node.is_ok(),
            "Error receiving protocol message: {:?}",
            find_node
        );
        let find_node = find_node.unwrap();
        assert!(
            find_node.is_some(),
            "Error receiving protocol message: {:?}",
            find_node
        );
        let (find_node, _) = find_node.unwrap();
        assert!(
            matches!(find_node, ProtocolMessage::FindNodeReq(_)),
            "Expected to send find node but sent this: {:?}",
            find_node
        );
        let nonce = find_node.nonce().cloned();
        assert!(
            nonce.is_some(),
            "Returned message without nonce: {:?}",
            find_node
        );
        let nonce = nonce.unwrap();

        // Max retries is 2 -> 2 FindNodes are sent. After the second error the contact gets removed.
        let event = UseCaseEvent::Message(
            ProtocolMessage::Error(ReqRspMessage {
                nonce: nonce.clone(),
                source_state_seq_nr: StateSeqNr::from(2),
                data: ErrorData::SegmentFailure {
                    failed_link: failed_link.clone(),
                    source: neighbor_id.clone(),
                },
                not_via: Default::default(),
                source_route: SourceRoute::from(Path::from([neighbor_id.clone(), root_id.clone()])),
            }),
            interface.clone(),
        );
        let handle_result = use_case.handle_event(&sync_context, event.clone());
        assert!(
            handle_result.is_ok(),
            "Failed to handle error message: {:?}",
            handle_result
        );

        let find_node = hub_receiver.try_recv().await;
        assert!(
            find_node.is_ok(),
            "Error receiving protocol message: {:?}",
            find_node
        );
        let find_node = find_node.unwrap();
        assert!(
            find_node.is_some(),
            "Error receiving protocol message: {:?}",
            find_node
        );
        let (find_node, _) = find_node.unwrap();
        assert!(
            matches!(find_node, ProtocolMessage::FindNodeReq(_)),
            "Expected a new find node req to be emitted"
        );
        let nonce = find_node.nonce().cloned();
        assert!(
            nonce.is_some(),
            "Returned message without nonce: {:?}",
            find_node
        );
        let nonce = nonce.unwrap();

        let event = UseCaseEvent::Message(
            ProtocolMessage::Error(ReqRspMessage {
                nonce: nonce.clone(),
                source_state_seq_nr: StateSeqNr::from(2),
                data: ErrorData::SegmentFailure {
                    failed_link,
                    source: neighbor_id.clone(),
                },
                not_via: Default::default(),
                source_route: SourceRoute::from(Path::from([neighbor_id.clone(), root_id.clone()])),
            }),
            interface.clone(),
        );
        let handle_result = use_case.handle_event(&sync_context, event.clone());
        assert!(
            handle_result.is_ok(),
            "Failed to handle error message: {:?}",
            handle_result
        );

        // Now the contact should be removed
        let routing_table = sync_context.routing_table();
        let stored_failed_contact = routing_table.contact(&failed_id);
        assert!(
            stored_failed_contact.is_none(),
            "Contact is still present after failure: {:?}",
            stored_failed_contact
        );
    }

    #[tokio::test]
    async fn timeouts_yield_new_find_node_until_max_retries_with_exponential_backoff() {
        crate::tests::init();

        let interface = NetworkInterface::new("test");

        let root_id = NodeId::with_lsb(1);
        let neighbor_id = NodeId::with_lsb(2);
        let failed_id = NodeId::with_lsb(3);

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(30);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, mut hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(2));
        let failed_contact_before = Contact::new(
            Path::from([neighbor_id.clone(), failed_id.clone()]),
            StateSeqNr::from(3),
        );
        let mut failed_contact_after = Contact::new(
            Path::from([neighbor_id.clone(), failed_id.clone()]),
            StateSeqNr::from(3),
        );
        *failed_contact_after.state_mut() = ContactState::Invalid;

        let mut routing_table = SingleBucketRT::<20>::new(root_id.clone());
        assert!(routing_table.insert(neighbor.clone()).is_ok());
        assert!(routing_table.insert(failed_contact_before.clone()).is_ok());

        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());

        let sync_context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table,
            insertion_strategy: TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            message_sender: hub_sender,
            runtime: runtime.clone(),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let config = FailureHandlingConfig {
            failure_notification_radius: NonZeroUsize::new(1).unwrap(),
            grouping_bits: NonZeroUsize::new(1).unwrap(),
            backoff_timeout_interval: RediscoveryTimeoutInterval::new(
                1f64,
                1f64,
                DistanceMap::new(
                    Duration::from_millis(500),
                    Duration::from_millis(500),
                    Duration::from_millis(500),
                ),
            )
            .unwrap(),
            backoff_max_retries: NonZeroU32::new(3).unwrap(),
        };
        let mut use_case = FailureHandling::new(config);
        assert!(
            use_case.start(&sync_context).is_ok(),
            "failed to start use case"
        );

        let event = UseCaseEvent::Contact(ContactEvent::Updated {
            old: failed_contact_before,
            new: failed_contact_after,
        });
        let result = use_case.handle_event(&sync_context, event);
        assert!(
            result.is_ok(),
            "Failed to handle contact invalidation event"
        );

        // First UpdateRoute is sent
        let update_route = hub_receiver.try_recv().await;
        assert!(
            matches!(
                update_route,
                Ok(Some((ProtocolMessage::UpdateRouteReq(_), _)))
            ),
            "First message is not a UpdateRouteReq: {:?}",
            update_route
        );

        // Then a FindNodeReq is sent
        let find_node = hub_receiver.try_recv().await;
        assert!(
            matches!(find_node, Ok(Some((ProtocolMessage::FindNodeReq(_), _)))),
            "Second message is not a FindNodeReq: {:?}",
            find_node
        );

        // 1. Timeout is triggered
        let timer_id = runtime.latest_timer_id();
        assert!(timer_id.is_some(), "No timer was created");
        let timer_id = timer_id.unwrap();

        assert_eq!(
            runtime.get_timer_duration(&timer_id),
            Some(Duration::from_millis(500))
        );

        // Max retries is 2 -> 2 FindNodes are sent. After the second error the contact gets removed.
        let event = UseCaseEvent::Timer(timer_id);
        let handle_result = use_case.handle_event(&sync_context, event.clone());
        assert!(
            handle_result.is_ok(),
            "Failed to handle error message: {:?}",
            handle_result
        );

        // Then a FindNodeReq is sent
        let find_node = hub_receiver.try_recv().await;
        assert!(
            matches!(find_node, Ok(Some((ProtocolMessage::FindNodeReq(_), _)))),
            "Second message is not a FindNodeReq: {:?}",
            find_node
        );

        // 2. timeout is triggered
        let timer_id = runtime.latest_timer_id();
        assert!(timer_id.is_some(), "No timer was created");
        let timer_id = timer_id.unwrap();

        assert_eq!(
            runtime.get_timer_duration(&timer_id),
            Some(Duration::from_millis(1000)),
            "Timer duration is not exponentially incremented"
        );

        let event = UseCaseEvent::Timer(timer_id);
        let handle_result = use_case.handle_event(&sync_context, event.clone());
        assert!(
            handle_result.is_ok(),
            "Failed to handle error message: {:?}",
            handle_result
        );

        // Then a FindNodeReq is sent
        let find_node = hub_receiver.try_recv().await;
        assert!(
            matches!(find_node, Ok(Some((ProtocolMessage::FindNodeReq(_), _)))),
            "Second message is not a FindNodeReq: {:?}",
            find_node
        );

        // 3. timeout is triggered
        let timer_id = runtime.latest_timer_id();
        assert!(timer_id.is_some(), "No timer was created");
        let timer_id = timer_id.unwrap();

        assert_eq!(
            runtime.get_timer_duration(&timer_id),
            Some(Duration::from_millis(2000)),
            "Timer duration is not exponentially incremented"
        );

        let event = UseCaseEvent::Timer(timer_id);
        let handle_result = use_case.handle_event(&sync_context, event.clone());
        assert!(
            handle_result.is_ok(),
            "Failed to handle error message: {:?}",
            handle_result
        );

        // Now the contact should be removed
        let routing_table = sync_context.routing_table();
        let stored_failed_contact = routing_table.contact(&failed_id);
        assert!(
            stored_failed_contact.is_none(),
            "Contact is still present after failure: {:?}",
            stored_failed_contact
        );
    }

    #[test]
    fn hardware_event_invalidates_all_affected_contacts() {
        crate::tests::init();

        let interface = NetworkInterface::new("test");
        let other_interface = NetworkInterface::new("test 2");

        let root_id = NodeId::with_lsb(1);
        let neighbor_id = NodeId::with_lsb(2);
        let over_neighbor_id = NodeId::with_lsb(3);
        let other_id = NodeId::with_lsb(4);

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(30);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(2));
        let over_neighbor_contact = Contact::new(
            Path::from([neighbor_id.clone(), over_neighbor_id.clone()]),
            StateSeqNr::from(3),
        );
        let other_contact = Contact::new(Path::from(other_id.clone()), StateSeqNr::from(4));

        let mut routing_table = SingleBucketRT::<20>::new(root_id.clone());
        assert!(routing_table.insert(neighbor.clone()).is_ok());
        assert!(routing_table.insert(over_neighbor_contact.clone()).is_ok());
        assert!(routing_table.insert(other_contact.clone()).is_ok());

        let mut pn_table = PNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());
        pn_table.insert(other_id.clone(), other_interface.clone());

        let sync_context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table,
            insertion_strategy: TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            message_sender: hub_sender,
            runtime: runtime.clone(),
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let config = FailureHandlingConfig {
            failure_notification_radius: NonZeroUsize::new(1).unwrap(),
            grouping_bits: NonZeroUsize::new(1).unwrap(),
            backoff_timeout_interval: Default::default(),
            backoff_max_retries: NonZeroU32::new(3).unwrap(),
        };
        let mut use_case = FailureHandling::new(config);
        assert!(
            use_case.start(&sync_context).is_ok(),
            "failed to start use case"
        );

        let mut interfaces = HashSet::new();
        interfaces.insert(interface.clone());
        let event = UseCaseEvent::Hardware(HardwareEvent::InterfacesDown(interfaces));
        let result = use_case.handle_event(&sync_context, event);
        assert!(
            result.is_ok(),
            "Failed to handle contact invalidation event"
        );

        let rt = sync_context.routing_table();
        assert_eq!(
            rt.contact(&neighbor_id).map(|contact| contact.state()),
            Some(&ContactState::Invalid),
            "affected neighbor should be invalidated"
        );
        assert_eq!(
            rt.contact(&over_neighbor_id).map(|contact| contact.state()),
            Some(&ContactState::Invalid),
            "affected contact should be invalidated"
        );
        assert_eq!(
            rt.contact(&other_id).map(|contact| contact.state()),
            Some(&ContactState::Valid),
            "non-affected contact should still be valid"
        );
    }
}
