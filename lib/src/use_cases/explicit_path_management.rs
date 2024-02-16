use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::marker::PhantomData;
use std::num::NonZeroUsize;
use std::ops::Deref;
use std::time::{Duration, Instant};

use crate::context::UseCaseContext;
use crate::domain::{Contact, ContactState, EmptyPathError, NetworkInterface, NodeId, Path, PathId, PNTable, RoutingTable};
use crate::forwarding::hasher::Hasher;
use crate::forwarding::{PathIdDecapsulationEntry, PathIdEntry, PathIdForwardingEntry, PathIdTable};
use crate::hardware_events::HardwareEvent;
use crate::messaging::source_route::SourceRoute;
use crate::messaging::{Nonce, PathSetupReqData, PathTeardownReqData, ProbeReqData, ProtocolMessage, ProtocolMessageSender, ReqRspMessage};
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{ContactEvent, EventHandler, HandlingResult, TimerId, UseCase, UseCaseEvent, UseCaseState};

/// Configuration for [ExplicitPathManagement] use case.
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
    /// Exclusive Vicinity radius from which to create PathSetupReq messages.
    ///
    /// # Example
    ///
    /// If the `vicinity_radius` is 3 then only for contacts with path size of at least 4 will
    /// yield a PathSetupReq.
    pub vicinity_radius: NonZeroUsize,
    /// Hasher to use for derivation of [PathIDs](crate::domain::path_id::PathId) from [Path]s.
    pub hasher: Hasher,
}

impl Default for EPMConfig {
    fn default() -> Self {
        Self {
            max_age: Duration::from_secs(60),
            cleanup_interval: Duration::from_secs(60),
            refresh_interval: Some(Duration::from_secs(20)),
            vicinity_radius: NonZeroUsize::new(3).unwrap(),
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
///     outside of vicinity).
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
pub struct ExplicitPathManagement<C, const BUCKET_SIZE: usize> {
    _pd: PhantomData<C>,
    state: EPMState,
    config: EPMConfig,
}

impl<C, const BUCKET_SIZE: usize> ExplicitPathManagement<C, BUCKET_SIZE> {
    pub fn new(config: EPMConfig) -> Self {
        Self {
            _pd: PhantomData::default(),
            state: EPMState::Initialized,
            config,
        }
    }
}

impl<C, const BUCKET_SIZE: usize> ExplicitPathManagement<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::MessageSender: ProtocolMessageSender,
    C::ForwardingTables: PathIdTable,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::PhysicalNeighborTable: PNTable + Deref<Target = HashMap<NodeId, NetworkInterface>>
{
    fn send_setup_req(&self, context: &C, contact: &Contact) -> Result<(), EPMError> {
        let source_route = SourceRoute::new(context.root_id().clone(), contact.path().clone());
        let message = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
            data: PathSetupReqData,
            not_via: context.not_via().clone(),
            source_route,
        };
        if let Err(e) = context.message_sender_mut().send_message(message) {
            log::error!(target: "explicit_path_management", "failed to send PathSetupReq for {}: {}", contact.path(), e);
            return Err(EPMError::MessageSendFailed);
        }

        Ok(())
    }

    fn send_probe_req(&self, context: &C, contact: &Contact) -> Result<(), EPMError> {
        let source_route = SourceRoute::new(context.root_id().clone(), contact.path().clone());
        let message = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
            data: ProbeReqData,
            not_via: context.not_via().clone(),
            source_route,
        };
        if let Err(e) = context.message_sender_mut().send_message(message) {
            log::error!(target: "explicit_path_management", "failed to send ProbeReq to refresh path {} : {}", contact.path(), e);
            return Err(EPMError::MessageSendFailed);
        }

        Ok(())
    }

    fn send_teardown_req(&self, context: &C, contact: &Contact) -> Result<(), EPMError> {
        let source_route = SourceRoute::new(context.root_id().clone(), contact.path().clone());
        let message = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: *context.pn_table().state_seq_nr(),
            data: PathTeardownReqData,
            not_via: context.not_via().clone(),
            source_route,
        };
        if let Err(e) = context.message_sender_mut().send_message(message) {
            log::error!(target: "explicit_path_management", "failed to send PathSetupReq for {}: {}", contact.path(), e);
            return Err(EPMError::MessageSendFailed);
        }

        Ok(())
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
            match context.forwarding_tables_mut().remove(&id) {
                Err(e) => {
                    // Only warning as this could mean the entry was already removed
                    log::warn!(target: "explicit_path_management", "failed to remove path_id entry for {}: {:?}", id, e);
                }
                Ok(None) => {
                    log::trace!(target: "explicit_path_management", "tried to remove invalid entry for {}", id);
                }
                Ok(Some(entry)) => {
                    log::trace!(target: "explicit_path_management", "Removed entry {}", entry);
                }
            }
        }
    }

    fn perform_refresh(&mut self, context: &C) -> Result<(), EPMError> {
        let mut some_failed = false;
        for contact in context.routing_table().iter() {
            // dont probe invalid or vicinity contacts
            if contact.state() != &ContactState::Valid ||
                contact.path().size() <= self.config.vicinity_radius.get()
            {
                continue;
            }

            if self.send_probe_req(context, contact).is_err() {
                some_failed = true;
            }
        }

        if some_failed {
            Err(EPMError::MessageSendFailed)
        } else {
            Ok(())
        }
    }

    fn register_path(&mut self, context: &C, source_route: SourceRoute) {
        let entries;
        let local_entries;
        match &mut self.state {
            EPMState::Running {
                externally_added_paths,
                local_paths,
                ..
            } => {
                entries = externally_added_paths;
                local_entries = local_paths;
            }
            _ => {
                log::warn!(target: "explicit_path_management", "Tried to register foreign path while not initialized");
                return;
            }
        };

        let in_path = source_route.remaining_path();
        let out_path =
            Result::<Path, EmptyPathError>::from_iter(in_path.clone().into_iter().skip(1))
                .map(Some)
                .unwrap_or_else(|_| None);

        let in_path_id = self.config.hasher.hash(&in_path);

        match out_path {
            Some(out_path) => {
                let out_interface = match context.pn_table().get(out_path.first()).cloned() {
                    Some(interface) => interface,
                    None => {
                        log::error!(target: "explicit_path_management", "Received PathSetupRequest for invalid physical neighbor {}; Ignoring", out_path.first());
                        return;
                    }
                };
                let out_path_id = self.config.hasher.hash(&out_path);
                let entry = PathIdEntry::Forward(PathIdForwardingEntry{
                    in_path_id: in_path_id.clone(),
                    out_path_id,
                    next_hop: out_path.first().clone(),
                });

                if entries.contains_key(&in_path_id) {
                    if let Err(e) = context.forwarding_tables_mut().update(entry) {
                        log::error!(target: "explicit_path_management", "Failed to update existing entry: {:?}", e);
                        return;
                    }
                    let entry = entries.get_mut(&in_path_id).unwrap();
                    entry.last_seen = Instant::now();
                } else {
                    if let Err(e) = context.forwarding_tables_mut().create(entry.clone()) {
                        log::error!(target: "explicit_path_management", "Failed to create new entry: {:?}", e);
                        return;
                    }
                    entries.insert(in_path_id, Entry {
                        path_id_entry: entry,
                        interface: out_interface,
                        last_seen: Instant::now(),
                    });
                }
            },
            None => {
                if !local_entries.contains(&in_path_id) {
                    let entry = PathIdEntry::Decapsulate(PathIdDecapsulationEntry {
                        in_path_id: in_path_id.clone(),
                        local_id: context.root_id().clone(),
                    });
                    if let Err(e) = context.forwarding_tables_mut().create(entry) {
                        log::error!(target: "explicit_path_management", "Failed to create new entry: {:?}", e);
                        return;
                    }
                    local_entries.insert(in_path_id);
                } else {
                    // NOTE: local entries are never updated to avoid useless updates to the underlying nftables map
                }
            },
        }
    }

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
        if let Err(e) = context.forwarding_tables_mut().remove(&in_path_id) {
            log::error!(target: "explicit_path_management", "Failed to remove PathIdEntry: {:?}", e);
        }
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
            *refresh_timer = self.config.refresh_interval
                .map(|i| context.runtime().register_timer(i));
        }
    }

    fn invalidate_all_over_interfaces(
        &mut self,
        context: &C,
        interfaces: HashSet<NetworkInterface>,
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
            .filter(|(_, Entry { interface, .. })| interfaces.contains(interface))
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in affected {
            entries.remove(&id);
            if let Err(e) = context.forwarding_tables_mut().remove(&id) {
                // Only warning as this could mean the entry was already removed
                log::warn!(target: "explicit_path_management", "Failed to remove PathIdEntry for path id {}: {:?}", id, e);
            }
        }
    }
}

impl<C, const BUCKET_SIZE: usize> EventHandler for ExplicitPathManagement<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    C::MessageSender: ProtocolMessageSender,
    C::ForwardingTables: PathIdTable,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::PhysicalNeighborTable: PNTable + Deref<Target = HashMap<NodeId, NetworkInterface>>
{
    type Context = C;
    type Error = EPMError;
    type Value = HandlingResult;

    fn handle_event(
        &mut self,
        context: &Self::Context,
        event: UseCaseEvent,
    ) -> Result<Self::Value, Self::Error> {
        log::trace!(target: "explicit_path_management", "Event {:?}", event);
        match (event, &self.state) {
            // ========== Contact Updates ==========
            // FIXME don't send teardown if we are uncertain if other nodes use the path
            // possible fix: rely solely on soft-state cleanup
            (UseCaseEvent::Contact(ContactEvent::New(contact)), _) => {
                if contact.path().size() > self.config.vicinity_radius.get() {
                    self.send_setup_req(context, &contact)?;
                }
            }
            (UseCaseEvent::Contact(ContactEvent::Updated { new, old }), _) => {
                match (
                    new.state(),
                    old.state(),
                    new.path().size() > self.config.vicinity_radius.get(),
                    old.path().size() > self.config.vicinity_radius.get(),
                ) {
                    (&ContactState::Valid, &ContactState::Invalid, true, _) => {
                        // Contact gets valid and is out of vicinity
                        self.send_setup_req(context, &new)?;
                    }
                    (&ContactState::Invalid, &ContactState::Valid, true, true) => {
                        // Contact gets invalid and is out of vicinity
                        // The old path was out of vicinity too
                        self.send_teardown_req(context, &old)?;
                        if old.path() != new.path() {
                            self.send_teardown_req(context, &new)?;
                        }
                    }
                    (&ContactState::Valid, &ContactState::Valid, true, false) => {
                        // Contacts path changes from in vicinity to out of vicinity
                        self.send_setup_req(context, &new)?;
                    }
                    (&ContactState::Valid, &ContactState::Valid, false, true) => {
                        // Contacts path changed from out of vicinity to inside vicinity
                        self.send_teardown_req(context, &old)?;
                    }
                    _ => {}
                }
            }
            (UseCaseEvent::Contact(ContactEvent::Removed(contact)), _) => {
                if contact.path().size() > self.config.vicinity_radius.get() {
                    self.send_teardown_req(context, &contact)?;
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
                    self.perform_refresh(context)?;
                    self.create_new_refresh_timer(context);
                }
            }
            // ========== Protocol Messages ==========
            (
                UseCaseEvent::Message(ProtocolMessage::PathSetupReq(req), _),
                EPMState::Running { .. },
            ) => {
                let unprocessed_hops = req.source_route.remaining_path().size();
                assert!(unprocessed_hops > self.config.vicinity_radius.get());

                // process current hop
                self.register_path(context, req.source_route.clone());
                let processed_hops = unprocessed_hops - 1;

                // vicinity already has paths precomputed => stop forwarding to vicinity
                if processed_hops <= self.config.vicinity_radius.get() {
                    log::trace!(
                        target: "explicit_path_management",
                        "Stop forwarding PathSetupReq inside our vicinity: {:?}",
                        req
                    );
                    return Ok(HandlingResult::Handled);
                }
            }
            (
                UseCaseEvent::Message(ProtocolMessage::PathTeardownReq(req), _),
                EPMState::Running { .. },
            ) => {
                let unprocessed_hops = req.source_route.remaining_path().size();
                assert!(unprocessed_hops > self.config.vicinity_radius.get());

                // process current hop
                self.teardown_path(context, req.clone());
                let processed_hops = unprocessed_hops - 1;

                // stop forwarding to vicinity
                if processed_hops <= self.config.vicinity_radius.get() {
                    log::trace!(
                        target: "explicit_path_management",
                        "Stop forwarding PathTeardownReq inside our vicinity: {:?}",
                        req
                    );
                    return Ok(HandlingResult::Handled);
                }
            },
            (
                UseCaseEvent::Message(ProtocolMessage::ProbeReq(req), _),
                EPMState::Running { externally_added_paths, .. },
            ) => {
                let unprocessed_hops = req.source_route.remaining_path().size();
                // not need to keep paths fresh inside our precomputed vicinity
                if unprocessed_hops <= self.config.vicinity_radius.get() {
                    // allow destination to respond
                    return Ok(HandlingResult::NotHandled);
                }

                // warning: this also post installs PathIds
                self.register_path(context, req.source_route)
            },
            // ========== Hardware Events ==========
            (UseCaseEvent::Hardware(HardwareEvent::InterfacesDown(interfaces)), _) => {
                self.invalidate_all_over_interfaces(context, interfaces);
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
    C::MessageSender: ProtocolMessageSender,
    C::ForwardingTables: PathIdTable,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::PhysicalNeighborTable: PNTable + Deref<Target = HashMap<NodeId, NetworkInterface>>
{
    type State = EPMState;

    fn start(&mut self, context: &Self::Context) -> Result<(), Self::Error> {
        if self.state != EPMState::Initialized {
            return Err(EPMError::AlreadyStarted);
        }

        // As the periodic tasks may
        let cleanup_timer_id = context
            .runtime()
            .register_timer(self.config.cleanup_interval);
        let refresh_timer_id = self.config.refresh_interval
            .map(|i| context.runtime().register_timer(i));
        self.state = EPMState::Running {
            refresh_timer: refresh_timer_id,
            cleanup_timer: cleanup_timer_id,
            externally_added_paths: HashMap::default(),
            local_paths: HashSet::default(),
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
    pub interface: NetworkInterface,
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
        local_paths: HashSet<PathId>,
    },
    /// The [UseCase] reached an unrecoverable error state.
    Error,
}

impl UseCaseState for EPMState {
    fn is_error(&self) -> bool {
        self == &Self::Error
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum EPMError {
    AlreadyStarted,
    MessageSendFailed,
}

impl Display for EPMError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyStarted => write!(f, "Use case was started multiple times"),
            Self::MessageSendFailed => write!(f, "Failed to send protocol message"),
        }
    }
}

impl Error for EPMError {}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::num::NonZeroUsize;
    use std::time::Duration;
    use crate::broadcaster::MPSCBroadcaster;

    use crate::context::{ContextConfig, SyncContext, UseCaseContext};
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::domain::{Contact, InsertionStrategyResult, NetworkInterface, NodeId, PNTable, Path, RoutingTable, StateSeqNr, TestInsertionStrategy, InMemoryPNTable};
    use crate::forwarding::hasher::Hasher;
    use crate::forwarding::in_memory_tables::InMemoryFwdTables;
    use crate::forwarding::{PathIdEntry, PathIdTable};
    use crate::messaging::source_route::SourceRoute;
    use crate::messaging::{
        AsyncProtocolMessageReceiver, InMemoryMessageChannel, Nonce, PathSetupReqData,
        PathTeardownReqData, ProtocolMessage, ReqRspMessage,
    };
    use crate::runtime::ImmediateRuntime;
    use crate::use_cases::explicit_path_management::{EPMConfig, ExplicitPathManagement};
    use crate::use_cases::{ContactEvent, EventHandler, HandlingResult, UseCase, UseCaseEvent};

    #[tokio::test]
    async fn path_setup_on_new_contact() {
        crate::tests::init();

        let interface = NetworkInterface::with_name("test");

        let root_id = NodeId::with_lsb(1);
        let neighbor_id = NodeId::with_lsb(2);
        let new_contact_id = NodeId::with_lsb(3);

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(30);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, mut hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(2));

        let new_contact = Contact::new(
            Path::from([
                neighbor_id.clone(),
                NodeId::with_msb(15),
                NodeId::with_msb(16),
                NodeId::with_msb(17),
                NodeId::with_msb(18),
                new_contact_id.clone(),
            ]),
            StateSeqNr::from(3),
        );

        let mut routing_table = SingleBucketRT::<20>::new(root_id.clone());
        assert!(routing_table.insert(neighbor.clone()).is_ok());

        let mut pn_table = InMemoryPNTable::new();
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

        let config = EPMConfig {
            max_age: Duration::from_secs(1),
            cleanup_interval: Duration::from_secs(1),
            refresh_interval: None,
            vicinity_radius: NonZeroUsize::new(2).unwrap(),
            hasher: Hasher::Sha1,
        };
        let mut use_case = ExplicitPathManagement::new(config);
        assert!(
            use_case.start(&sync_context).is_ok(),
            "failed to start use case"
        );

        let event = UseCaseEvent::Contact(ContactEvent::New(new_contact.clone()));
        let result = use_case.handle_event(&sync_context, event);
        assert!(
            result.is_ok(),
            "Failed to handle contact invalidation event"
        );

        // First UpdateRoute is sent
        let path_setup_req = hub_receiver.try_recv().await;
        assert!(
            path_setup_req.is_ok(),
            "failed to receive path setup request: {:?}",
            path_setup_req
        );
        let path_setup_req = path_setup_req.unwrap();
        assert!(
            path_setup_req.is_some(),
            "no message received: {:?}",
            path_setup_req
        );
        let (path_setup_req, _) = path_setup_req.unwrap();
        if let ProtocolMessage::PathSetupReq(req) = path_setup_req {
            assert_eq!(
                &req.source_route,
                &SourceRoute::from(Path::from([
                    root_id.clone(),
                    neighbor_id.clone(),
                    NodeId::with_msb(15),
                    NodeId::with_msb(16),
                    NodeId::with_msb(17),
                    NodeId::with_msb(18),
                    new_contact_id.clone(),
                ]))
            )
        } else {
            panic!(
                "Invalid message returned by use case. Expected PathSetupReq, got: {:?}",
                path_setup_req
            );
        }
    }

    #[tokio::test]
    async fn path_teardown_on_removed_contact() {
        crate::tests::init();

        let interface = NetworkInterface::with_name("test");

        let root_id = NodeId::with_lsb(1);
        let neighbor_id = NodeId::with_lsb(2);
        let removed_contact_id = NodeId::with_lsb(3);

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(30);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, mut hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(2));

        let removed_contact = Contact::new(
            Path::from([
                neighbor_id.clone(),
                NodeId::with_msb(15),
                NodeId::with_msb(16),
                NodeId::with_msb(17),
                NodeId::with_msb(18),
                removed_contact_id.clone(),
            ]),
            StateSeqNr::from(3),
        );

        let mut routing_table = SingleBucketRT::<20>::new(root_id.clone());
        assert!(routing_table.insert(neighbor.clone()).is_ok());

        let mut pn_table = InMemoryPNTable::new();
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

        let config = EPMConfig {
            max_age: Duration::from_secs(1),
            cleanup_interval: Duration::from_secs(1),
            refresh_interval: None,
            vicinity_radius: NonZeroUsize::new(2).unwrap(),
            hasher: Hasher::Sha1,
        };
        let mut use_case = ExplicitPathManagement::new(config);
        assert!(
            use_case.start(&sync_context).is_ok(),
            "failed to start use case"
        );

        let event = UseCaseEvent::Contact(ContactEvent::Removed(removed_contact.clone()));
        let result = use_case.handle_event(&sync_context, event);
        assert!(
            result.is_ok(),
            "Failed to handle contact invalidation event"
        );

        // First UpdateRoute is sent
        let path_teardown_req = hub_receiver.try_recv().await;
        assert!(
            path_teardown_req.is_ok(),
            "failed to receive path setup request: {:?}",
            path_teardown_req
        );
        let path_teardown_req = path_teardown_req.unwrap();
        assert!(
            path_teardown_req.is_some(),
            "no message received: {:?}",
            path_teardown_req
        );
        let (path_teardown_req, _) = path_teardown_req.unwrap();
        if let ProtocolMessage::PathTeardownReq(req) = path_teardown_req {
            assert_eq!(
                &req.source_route,
                &SourceRoute::from(Path::from([
                    root_id.clone(),
                    neighbor_id.clone(),
                    NodeId::with_msb(15),
                    NodeId::with_msb(16),
                    NodeId::with_msb(17),
                    NodeId::with_msb(18),
                    removed_contact_id.clone(),
                ]))
            )
        } else {
            panic!(
                "Invalid message returned by use case. Expected PathSetupReq, got: {:?}",
                path_teardown_req
            );
        }
    }

    #[tokio::test]
    async fn incoming_path_setup_create_fwd_table_entry() {
        crate::tests::init();

        let interface = NetworkInterface::with_name("test");
        let interface2 = NetworkInterface::with_name("test 2");

        let root_id = NodeId::with_lsb(1);
        let neighbor_id = NodeId::with_lsb(2);
        let sender_contact_id = NodeId::with_lsb(3);
        let other_neighbor_id = NodeId::with_lsb(4);

        let setup_path = Path::from([
            sender_contact_id.clone(),
            NodeId::with_lsb(5),
            NodeId::with_lsb(6),
            NodeId::with_lsb(7),
            NodeId::with_lsb(8),
            neighbor_id.clone(),
            root_id.clone(),
            other_neighbor_id.clone(),
            NodeId::with_lsb(9),
            NodeId::with_lsb(10),
        ]);
        let setup_route = SourceRoute::from(setup_path)
            .advanced()
            .advanced()
            .advanced()
            .advanced()
            .advanced();
        let path_before = Path::from([
            root_id.clone(),
            other_neighbor_id.clone(),
            NodeId::with_lsb(9),
            NodeId::with_lsb(10),
        ]);
        let input_path_id = Hasher::Sha1.hash(&path_before);

        let path_after = Path::from([
            other_neighbor_id.clone(),
            NodeId::with_lsb(9),
            NodeId::with_lsb(10),
        ]);
        let output_path_id = Hasher::Sha1.hash(&path_after);
        // FIXME
        let path_id_entry = PathIdEntry {
            in_path_id: input_path_id.clone(),
            out_path_id: Some(output_path_id),
            next_hop: other_neighbor_id.clone(),
        };

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(30);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(2));
        let other_neighbor =
            Contact::new(Path::from(other_neighbor_id.clone()), StateSeqNr::from(4));

        let mut routing_table = SingleBucketRT::<20>::new(root_id.clone());
        assert!(routing_table.insert(neighbor.clone()).is_ok());
        assert!(routing_table.insert(other_neighbor.clone()).is_ok());

        let mut pn_table = InMemoryPNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());
        pn_table.insert(other_neighbor_id.clone(), interface2.clone());

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

        let config = EPMConfig {
            max_age: Duration::from_secs(1),
            cleanup_interval: Duration::from_secs(1),
            refresh_interval: None,
            vicinity_radius: NonZeroUsize::new(2).unwrap(),
            hasher: Hasher::Sha1,
        };
        let mut use_case = ExplicitPathManagement::new(config);
        assert!(
            use_case.start(&sync_context).is_ok(),
            "failed to start use case"
        );

        let req = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: StateSeqNr::from(4),
            data: PathSetupReqData,
            not_via: Default::default(),
            source_route: setup_route.clone(),
        };
        let event = UseCaseEvent::Message(ProtocolMessage::PathSetupReq(req.clone()), interface);
        let result = use_case.handle_event(&sync_context, event);
        assert!(
            result.is_ok(),
            "Failed to handle contact invalidation event"
        );

        // Forwarding table entry is created
        // Delegation is done in ForwardProtocolMessage UseCase
        let fwd_tables = sync_context.forwarding_tables();
        let entry = fwd_tables.path_id_entry(&input_path_id);
        assert!(entry.is_some(), "No entry for input path id created");
        let entry = entry.unwrap();
        assert_eq!(entry, &path_id_entry, "Invalid PathIdEntry created!");
    }

    #[tokio::test]
    async fn incoming_path_teardown_removes_fwd_table_entry() {
        crate::tests::init();

        let interface = NetworkInterface::with_name("test");
        let interface2 = NetworkInterface::with_name("test 2");

        let root_id = NodeId::with_lsb(1);
        let neighbor_id = NodeId::with_lsb(2);
        let sender_contact_id = NodeId::with_lsb(3);
        let other_neighbor_id = NodeId::with_lsb(4);

        let setup_path = Path::from([
            sender_contact_id.clone(),
            NodeId::with_lsb(5),
            NodeId::with_lsb(6),
            NodeId::with_lsb(7),
            NodeId::with_lsb(8),
            neighbor_id.clone(),
            root_id.clone(),
            other_neighbor_id.clone(),
            NodeId::with_lsb(9),
            NodeId::with_lsb(10),
        ]);
        let setup_route = SourceRoute::from(setup_path)
            .advanced()
            .advanced()
            .advanced()
            .advanced()
            .advanced();
        let path_before = Path::from([
            root_id.clone(),
            other_neighbor_id.clone(),
            NodeId::with_lsb(9),
            NodeId::with_lsb(10),
        ]);
        let input_path_id = Hasher::Sha1.hash(&path_before);

        let path_after = Path::from([
            other_neighbor_id.clone(),
            NodeId::with_lsb(9),
            NodeId::with_lsb(10),
        ]);
        let output_path_id = Hasher::Sha1.hash(&path_after);
        // FIXME
        let path_id_entry = PathIdEntry {
            in_path_id: input_path_id.clone(),
            out_path_id: Some(output_path_id),
            next_hop: other_neighbor_id.clone(),
        };

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(30);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let neighbor = Contact::new(Path::from(neighbor_id.clone()), StateSeqNr::from(2));
        let other_neighbor =
            Contact::new(Path::from(other_neighbor_id.clone()), StateSeqNr::from(4));

        let mut routing_table = SingleBucketRT::<20>::new(root_id.clone());
        assert!(routing_table.insert(neighbor.clone()).is_ok());
        assert!(routing_table.insert(other_neighbor.clone()).is_ok());

        let mut pn_table = InMemoryPNTable::new();
        pn_table.insert(neighbor_id.clone(), interface.clone());
        pn_table.insert(other_neighbor_id.clone(), interface2.clone());

        let mut forwarding_tables = InMemoryFwdTables::new();
        assert!(forwarding_tables.create(path_id_entry).is_ok());

        let sync_context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table,
            insertion_strategy: TestInsertionStrategy::from(InsertionStrategyResult::Inserted),
            message_sender: hub_sender,
            runtime: runtime.clone(),
            forwarding_tables,
            not_via: HashSet::default(),
        });

        let config = EPMConfig {
            max_age: Duration::from_secs(1),
            cleanup_interval: Duration::from_secs(1),
            refresh_interval: None,
            vicinity_radius: NonZeroUsize::new(2).unwrap(),
            hasher: Hasher::Sha1,
        };
        let mut use_case = ExplicitPathManagement::new(config);
        assert!(
            use_case.start(&sync_context).is_ok(),
            "failed to start use case"
        );

        let req = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: StateSeqNr::from(4),
            data: PathTeardownReqData,
            not_via: Default::default(),
            source_route: setup_route.clone(),
        };
        let event = UseCaseEvent::Message(ProtocolMessage::PathTeardownReq(req.clone()), interface);
        let result = use_case.handle_event(&sync_context, event);
        assert!(
            result.is_ok(),
            "Failed to handle contact invalidation event"
        );

        // Forwarding table entry is created
        // Delegation is done in ForwardProtocolMessage UseCase
        let fwd_tables = sync_context.forwarding_tables();
        let entry = fwd_tables.path_id_entry(&input_path_id);
        assert!(entry.is_none(), "Path id entry was not removed");
    }

    #[test]
    fn dont_forward_path_teardown_to_vicinity() {
        crate::tests::init();

        let interface = NetworkInterface::with_name("test");
        let interface2 = NetworkInterface::with_name("test 2");

        let root_id = NodeId::with_lsb(1);
        let neighbor_id = NodeId::with_lsb(2);
        let sender_contact_id = NodeId::with_lsb(3);
        let other_neighbor_id = NodeId::with_lsb(4);

        let setup_path = Path::from([
            sender_contact_id.clone(),
            NodeId::with_lsb(5),
            NodeId::with_lsb(6),
            NodeId::with_lsb(7),
            NodeId::with_lsb(8),
            neighbor_id.clone(),
            root_id.clone(),
            other_neighbor_id.clone(),
            NodeId::with_lsb(9),
            NodeId::with_lsb(10),
        ]);
        let setup_route = SourceRoute::from(setup_path)
            .advanced()
            .advanced()
            .advanced()
            .advanced()
            .advanced();

        let routing_table = SingleBucketRT::<20>::new(root_id.clone());

        let pn_table = InMemoryPNTable::new();

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(30);

        let runtime = ImmediateRuntime::new(broadcaster);

        let context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table,
            insertion_strategy,
            message_sender: hub_sender,
            runtime,
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let config = EPMConfig {
            max_age: Duration::from_secs(1),
            cleanup_interval: Duration::from_secs(1),
            refresh_interval: None,
            vicinity_radius: NonZeroUsize::new(3).unwrap(),
            hasher: Hasher::Sha1,
        };
        let mut use_case = ExplicitPathManagement::new(config);
        assert!(
            use_case.start(&context).is_ok(),
            "failed to start use case"
        );

        let req = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: StateSeqNr::from(4),
            data: PathTeardownReqData,
            not_via: Default::default(),
            source_route: setup_route.clone(),
        };
        let message = ProtocolMessage::PathTeardownReq(req.clone());
        let event = UseCaseEvent::Message(message.clone(), interface);
        let result = use_case.handle_event(&context, event);
        assert!(
            result.is_ok(),
            "Failed to handle contact invalidation event"
        );

        let result = result.unwrap();
        assert!(
            matches!(
                result,
                HandlingResult::Handled
            ),
            "Failed to stop forwarding in the vicinity for message {:?}",
            message
        )
    }

    #[test]
    fn dont_forward_path_setup_to_vicinity() {
        crate::tests::init();

        let interface = NetworkInterface::with_name("test");
        let interface2 = NetworkInterface::with_name("test 2");

        let root_id = NodeId::with_lsb(1);
        let neighbor_id = NodeId::with_lsb(2);
        let sender_contact_id = NodeId::with_lsb(3);
        let other_neighbor_id = NodeId::with_lsb(4);

        let setup_path = Path::from([
            sender_contact_id.clone(),
            NodeId::with_lsb(5),
            NodeId::with_lsb(6),
            NodeId::with_lsb(7),
            NodeId::with_lsb(8),
            neighbor_id.clone(),
            root_id.clone(),
            other_neighbor_id.clone(),
            NodeId::with_lsb(9),
            NodeId::with_lsb(10),
        ]);
        let setup_route = SourceRoute::from(setup_path)
            .advanced()
            .advanced()
            .advanced()
            .advanced()
            .advanced();

        let routing_table = SingleBucketRT::<20>::new(root_id.clone());

        let pn_table = InMemoryPNTable::new();

        let insertion_strategy = TestInsertionStrategy::from(InsertionStrategyResult::Inserted);

        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let (broadcaster, _broadcast_receiver) = MPSCBroadcaster::new(30);

        let runtime = ImmediateRuntime::new(broadcaster);

        let context = SyncContext::new(ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            pn_table,
            insertion_strategy,
            message_sender: hub_sender,
            runtime,
            forwarding_tables: InMemoryFwdTables::new(),
            not_via: HashSet::default(),
        });

        let config = EPMConfig {
            max_age: Duration::from_secs(1),
            cleanup_interval: Duration::from_secs(1),
            refresh_interval: None,
            vicinity_radius: NonZeroUsize::new(3).unwrap(),
            hasher: Hasher::Sha1,
        };
        let mut use_case = ExplicitPathManagement::new(config);
        assert!(
            use_case.start(&context).is_ok(),
            "failed to start use case"
        );

        let req = ReqRspMessage {
            nonce: Nonce::random(),
            source_state_seq_nr: StateSeqNr::from(4),
            data: PathSetupReqData,
            not_via: Default::default(),
            source_route: setup_route.clone(),
        };
        let message = ProtocolMessage::PathSetupReq(req.clone());
        let event = UseCaseEvent::Message(message.clone(), interface);
        let result = use_case.handle_event(&context, event);
        assert!(
            result.is_ok(),
            "Failed to handle contact invalidation event"
        );

        let result = result.unwrap();
        assert!(
            matches!(
                result,
                HandlingResult::Handled
            ),
            "Failed to stop forwarding in the vicinity for message {:?}",
            message
        )
    }
}
