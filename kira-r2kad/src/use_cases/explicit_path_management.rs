use std::collections::HashMap;
use std::collections::hash_map::Entry::{Occupied, Vacant};
use std::marker::PhantomData;
use std::ops::Deref;
use std::time::{Duration, Instant};

use derive_more::derive::{Display, Error};
use tracing::{Level, instrument};

use crate::domain::protocol_event::forwarding::{
    PathIdEntry, PathIdForwardingEntry, PathIdTableUpdate,
};
use crate::domain::{
    Contact, ContactState, NodeId, Path, PathId, RoutingTable, ULNTable, UnderlayNeighborId,
    VICINITY_RADIUS, hasher::Hasher,
};
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{
    CommonHeader, PathSetupReqData, PathTeardownReqData, ProbeReqData, ProtocolMessage,
    ProtocolMessageKind, ReqRspMessage,
};
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{
    ContactEvent, EventHandler, HandlingResult, TimerId, UseCase, UseCaseContext, UseCaseEvent,
    UseCaseState,
};

/// Configuration for [ExplicitPathManagement] use case.
#[derive(Debug)]
pub struct EPMConfig {
    /// Maximum age of an externally added [PathIdEntry].
    pub max_age: Duration,
    /// Interval to perform cleanup of teared down or old entries.
    pub cleanup_interval: Duration,
    /// Interval to perform path setup refreshes.
    ///
    /// If [None] is passed no active refresh is performed.
    /// Still this use case uses ProbeReq sent out by to refresh its paths.
    pub refresh_interval: Option<Duration>,
    /// Hasher to use for derivation of [PathIDs](crate::domain::path_id::PathId) from [Path]s.
    pub hasher: Hasher,
}

impl Default for EPMConfig {
    fn default() -> Self {
        Self {
            max_age: Duration::from_secs(60),
            cleanup_interval: Duration::from_secs(60),
            refresh_interval: Some(Duration::from_secs(20)),
            hasher: Hasher::default(),
        }
    }
}

/// Represents the explicit path management use case.
///
/// Implements the PathSetup and Teardown for valid contacts in the routing table.
/// This includes:
///
/// - Periodic PathSetup to keep the soft state in intermediate systems alive (only for paths
///   outside of vicinity).
/// - PathTeardown for contacts that are removed from or invalidated in the routing table.
/// - Handling of incoming PathSetup and PathTeardowns.
/// - Periodic cleanup and removal of old entries in the forwarding table.
/// - Invalidation of entries affected by hardware event.
///
/// Given that even intermediate nodes are required to manage PathSetup and PathTeardown requests,
/// **it is necessary for this [UseCase] to be executed before the [ForwardProtocolMessage](super::forward_protocol_message::ForwardProtocolMessage) [UseCase]**.
///
/// This [UseCase] will return [HandlingResult::Handled] if no further forwarding by the
/// [ForwardProtocolMessage](super::forward_protocol_message::ForwardProtocolMessage) [UseCase] is necessary.
#[derive(Debug)]
pub struct ExplicitPathManagement<C, const BUCKET_SIZE: usize> {
    _pd: PhantomData<C>,
    state: EPMState,
    config: EPMConfig,
}

impl<C, const BUCKET_SIZE: usize> ExplicitPathManagement<C, BUCKET_SIZE> {
    pub fn new(config: EPMConfig) -> Self {
        Self {
            _pd: PhantomData,
            state: EPMState::Initialized,
            config,
        }
    }
}

impl<C, const BUCKET_SIZE: usize> ExplicitPathManagement<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    fn send_setup_req(&self, context: &C, contact: &Contact) {
        // just in case the contact has been invalidated meanwhile
        if !contact.is_valid() {
            return;
        }
        let source_route = SourceRoute::new(*context.root_id(), contact.path().unwrap().clone());
        let message = ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::PathSetupReq,
                *context.root_id(),
                *source_route.destination(),
                None,
                Some(From::from(*context.uln_table().state_seq_nr())),
                context.uln_table().size(),
            ),
            data: PathSetupReqData,
            not_via: None,
            source_route,
        };
        context
            .runtime()
            .send_message(message, context.uln_table().deref());
    }

    fn send_probe_req(&self, context: &C, contact: &Contact) {
        let source_route = SourceRoute::new(
            *context.root_id(),
            contact
                .path()
                .expect("ProbeReq for refreshing contact should be called for valid contacts only")
                .clone(),
        );
        let message = ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::ProbeReq,
                *context.root_id(),
                *source_route.destination(),
                None,
                Some(From::from(*context.uln_table().state_seq_nr())),
                context.uln_table().size(),
            ),
            data: ProbeReqData,
            not_via: None,
            source_route,
        };
        context
            .runtime()
            .send_message(message, context.uln_table().deref());
    }

    fn send_teardown_req(&self, context: &C, contact: &Contact) {
        let Some(active_path) = contact.path() else {
            return;
        };
        let source_route = SourceRoute::new(*context.root_id(), active_path.clone());
        let message = ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::PathTeardownReq,
                *context.root_id(),
                *source_route.destination(),
                None,
                Some(From::from(*context.uln_table().state_seq_nr())),
                context.uln_table().size(),
            ),
            data: PathTeardownReqData,
            not_via: None,
            source_route,
        };
        context
            .runtime()
            .send_message(message, context.uln_table().deref());
    }

    // Only deletes paths setup by others.
    fn perform_cleanup(&mut self, context: &C) {
        let entries = match &mut self.state {
            EPMState::Running {
                externally_added_paths,
                ..
            } => externally_added_paths,
            _ => {
                log::warn!(target: "explicit_path_management", "Tried to perform cleanup while not initialized");
                return;
            }
        };

        let invalidated_entries = entries
            .iter()
            // Only include entries which have not been updated lately
            .filter_map(|(id, entry)| {
                if entry.last_seen.elapsed() < self.config.max_age {
                    None
                } else {
                    Some(id.clone())
                }
            })
            .collect::<Vec<_>>();
        for id in invalidated_entries {
            entries.remove(&id);
            context
                .runtime()
                .update_fwd_tables(PathIdTableUpdate::Remove(id));
        }
    }

    fn perform_refresh(&mut self, context: &C) {
        for contact in context.routing_table().iter() {
            // don't probe invalid or vicinity contacts
            if contact.state() != &ContactState::Valid
                || contact.path().unwrap().size() <= VICINITY_RADIUS
            {
                continue;
            }

            self.send_probe_req(context, contact);
        }
    }

    fn register_path(&mut self, context: &C, source_route: SourceRoute) {
        let EPMState::Running {
            ref mut externally_added_paths,
            ..
        } = self.state
        else {
            log::warn!(target: "explicit_path_management", "Tried to register foreign path while not initialized");
            return;
        };

        let in_path = source_route.remaining_path();
        debug_assert!(in_path.size() > VICINITY_RADIUS);
        let out_path = Result::<Path, _>::from_iter(in_path.clone().into_iter().skip(1))
            .expect("should contain path >= {VICINITY_RADIUS}");

        let in_path_id = self.config.hasher.hash(&in_path);

        let next_hop = match context.uln_table().get(out_path.first()).cloned() {
            Some(next_hop) => next_hop,
            None => {
                log::error!(target: "explicit_path_management", "Received PathSetupRequest for invalid underlay neighbor {}; Ignoring", out_path.first());
                return;
            }
        };
        let out_path_id = self.config.hasher.hash(&out_path);
        let entry = PathIdEntry::Forward(PathIdForwardingEntry {
            in_path_id: in_path_id.clone(),
            out_path_id,
            next_hop,
        });

        match externally_added_paths.entry(in_path_id) {
            Occupied(mut occupied_entry) => {
                context
                    .runtime()
                    .update_fwd_tables(PathIdTableUpdate::Update(entry));
                occupied_entry.get_mut().last_seen = context.runtime().current_time();
            }
            Vacant(vacant_entry) => {
                context
                    .runtime()
                    .update_fwd_tables(PathIdTableUpdate::Create(entry.clone()));

                vacant_entry.insert(Entry {
                    path_id_entry: entry,
                    via: next_hop,
                    last_seen: context.runtime().current_time(),
                });
            }
        }
    }

    #[allow(dead_code)]
    fn teardown_path(&mut self, context: &C, req: ReqRspMessage<PathTeardownReqData>) {
        let entries = match &mut self.state {
            EPMState::Running {
                externally_added_paths,
                ..
            } => externally_added_paths,
            _ => {
                log::warn!(target: "explicit_path_management", "Tried to register foreign path while not initialized");
                return;
            }
        };
        let in_path = req.source_route.remaining_path();
        let in_path_id = self.config.hasher.hash(&in_path);

        entries.remove(&in_path_id);
        context
            .runtime()
            .update_fwd_tables(PathIdTableUpdate::Remove(in_path_id));
    }

    fn create_new_cleanup_timer(&mut self, context: &C) {
        if let EPMState::Running { cleanup_timer, .. } = &mut self.state {
            *cleanup_timer = context
                .runtime()
                .register_timer(self.config.cleanup_interval);
        }
    }

    fn create_new_refresh_timer(&mut self, context: &C) {
        if let EPMState::Running { refresh_timer, .. } = &mut self.state {
            *refresh_timer = self
                .config
                .refresh_interval
                .map(|i| context.runtime().register_timer(i));
        }
    }

    fn invalidate_all_over_interfaces(
        &mut self,
        context: &C,
        affected_neighbor: &UnderlayNeighborId,
    ) {
        let entries = match &mut self.state {
            EPMState::Running {
                externally_added_paths,
                ..
            } => externally_added_paths,
            _ => {
                log::warn!(target: "explicit_path_management", "Tried to register foreign path while not initialized");
                return;
            }
        };
        let affected = entries
            .iter()
            .filter(|(_, Entry { via, .. })| via == affected_neighbor)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in affected {
            entries.remove(&id);
            context
                .runtime()
                .update_fwd_tables(PathIdTableUpdate::Remove(id));
        }
    }
}

impl<C, const BUCKET_SIZE: usize> EventHandler for ExplicitPathManagement<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    type Context = C;
    type Error = EPMError;
    type Value = HandlingResult;

    #[instrument(
        level = Level::TRACE,
        target = "explicit_path_management",
        "explicit_path_management",
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
        match (event, &self.state) {
            // ========== Contact Events ==========
            (UseCaseEvent::Contact(contact_event), _) => {
                match *contact_event {
                    ContactEvent::New(contact)
                        if contact.path().unwrap().size() > VICINITY_RADIUS =>
                    {
                        // new contact outside vicinity requires path setup
                        self.send_setup_req(context, &contact);
                    }
                    ContactEvent::Updated { new, old } => {
                        match (
                            new.state(),
                            old.state(),
                            new.path().unwrap().size() > VICINITY_RADIUS,
                            old.path().unwrap().size() > VICINITY_RADIUS,
                        ) {
                            (ContactState::Valid, ContactState::Invalid(_), true, _)
                            | (ContactState::Valid, ContactState::Rediscovering(_), true, _) => {
                                // Contacts becomes valid
                                self.send_setup_req(context, &new);
                            }
                            (ContactState::Valid, ContactState::Valid, true, false) => {
                                // Contacts path changes from in vicinity to out of vicinity
                                self.send_setup_req(context, &new);
                            }
                            (ContactState::Valid, ContactState::Valid, true, true)
                                if old.path() != new.path() =>
                            {
                                // path changes outside the vicinity
                                self.send_setup_req(context, &new);
                                self.send_teardown_req(context, &old);
                            }
                            (ContactState::Valid, &ContactState::Valid, false, true) => {
                                // Contacts path changed from out of vicinity to inside vicinity
                                self.send_teardown_req(context, &old);
                            }
                            (
                                ContactState::Invalid(_),
                                ContactState::Valid,
                                new_outside_vicinity,
                                old_outside_vicinity,
                            ) => {
                                // Contact gets invalid
                                if new_outside_vicinity {
                                    self.send_teardown_req(context, &old);
                                }
                                if old_outside_vicinity && old.path() != new.path() {
                                    self.send_teardown_req(context, &new);
                                }
                            }
                            _ => {
                                // Ignored Cases:
                                // 1. Contact changes while being invalid
                                // 2. Contact changes inside vicinity
                                // 3. Contact changes outside vicinity without path change
                                // 4. Contact gets valid inside vincity
                            }
                        }
                    }
                    ContactEvent::Removed(contact)
                        if contact.path().unwrap().size() > VICINITY_RADIUS =>
                    {
                        self.send_teardown_req(context, &contact);
                    }
                    _ => {}
                }
            }
            // ========== Timers ==========
            (
                UseCaseEvent::Timer(timer),
                EPMState::Running {
                    cleanup_timer,
                    refresh_timer,
                    ..
                },
            ) => {
                if &timer == cleanup_timer {
                    self.perform_cleanup(context);
                    self.create_new_cleanup_timer(context);
                } else if &Some(timer) == refresh_timer {
                    self.perform_refresh(context);
                    self.create_new_refresh_timer(context);
                }
            }
            // ========== Protocol Messages ==========
            (
                UseCaseEvent::Message(ProtocolMessage::PathSetupReq(req), _),
                EPMState::Running { .. },
            ) => {
                let unprocessed_hops = req.source_route.remaining_path().size(); // X, [A], B, C, D -> 4
                assert!(unprocessed_hops > VICINITY_RADIUS);

                // process current hop
                self.register_path(context, req.source_route.clone());
                let processed_hops = unprocessed_hops - 1;

                // vicinity already has paths precomputed => stop forwarding to vicinity
                if processed_hops <= VICINITY_RADIUS {
                    log::trace!(
                        target: "explicit_path_management",
                        "Stop forwarding PathSetupReq inside our vicinity: {req:?}"
                    );
                    return Ok(HandlingResult::Handled);
                }
            }
            (
                UseCaseEvent::Message(ProtocolMessage::PathTeardownReq(req), _),
                EPMState::Running { .. },
            ) => {
                let unprocessed_hops = req.source_route.remaining_path().size();
                assert!(unprocessed_hops > VICINITY_RADIUS);

                // process current hop
                // WARN: insufficient sign to teardown path, because
                //       others may utilize the same path unknowingly.
                //       Let softstate perform the garbage collection.

                //self.teardown_path(context, req.clone());
                let processed_hops = unprocessed_hops - 1;

                // stop forwarding to vicinity
                if processed_hops <= VICINITY_RADIUS {
                    log::trace!(
                        target: "explicit_path_management",
                        "Stop forwarding PathTeardownReq inside our vicinity: {req:?}"
                    );
                    return Ok(HandlingResult::Handled);
                }
            }
            (
                UseCaseEvent::Message(ProtocolMessage::ProbeReq(req), _),
                EPMState::Running { .. },
            ) => {
                let unprocessed_hops = req.source_route.remaining_path().size();
                // not need to keep paths fresh inside our precomputed vicinity
                if unprocessed_hops <= VICINITY_RADIUS {
                    // allow destination to respond
                    return Ok(HandlingResult::NotHandled);
                }

                // warning: this also post installs PathIds
                self.register_path(context, req.source_route)
            }
            // ========== Hardware Events ==========
            (
                UseCaseEvent::UnderlayUpdate(
                    crate::domain::UnderlayNeighborUpdate::UnderlayNeighborDown(ref ulnid),
                ),
                _,
            ) => {
                self.invalidate_all_over_interfaces(context, ulnid);
            }
            _ => {}
        }

        Ok(HandlingResult::NotHandled)
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase for ExplicitPathManagement<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable: ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    type State = EPMState;

    fn start(&mut self, context: &C) -> Result<(), Self::Error> {
        if self.state != EPMState::Initialized {
            return Err(EPMError::AlreadyStarted);
        }

        // As the periodic tasks may
        let cleanup_timer_id = context
            .runtime()
            .register_timer(self.config.cleanup_interval);
        let refresh_timer_id = self
            .config
            .refresh_interval
            .map(|i| context.runtime().register_timer(i));
        self.state = EPMState::Running {
            refresh_timer: refresh_timer_id,
            cleanup_timer: cleanup_timer_id,
            externally_added_paths: HashMap::default(),
        };

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Entry {
    pub path_id_entry: PathIdEntry,
    pub via: UnderlayNeighborId,
    pub last_seen: Instant,
}

#[derive(Debug, Eq, PartialEq, Clone, Default)]
pub enum EPMState {
    /// The [UseCase] is waiting for startup.
    #[default]
    Initialized,
    /// The [UseCase] is running and has periodic garbage collection.
    Running {
        refresh_timer: Option<TimerId>,
        cleanup_timer: TimerId,
        externally_added_paths: HashMap<PathId, Entry>,
    },
    /// The [UseCase] reached an unrecoverable error state.
    Error,
}

impl UseCaseState for EPMState {
    fn is_error(&self) -> bool {
        self == &Self::Error
    }
}

#[derive(Debug, Eq, PartialEq, Clone, Display, Error)]
pub enum EPMError {
    #[display("Use case was started multiple times")]
    AlreadyStarted,
}
