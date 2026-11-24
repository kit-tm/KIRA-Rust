use std::{
    collections::HashMap,
    marker::PhantomData,
    ops::Deref,
    time::Duration,
};

use derive_more::derive::{
    Display,
    Error,
};
use tracing::{
    Level,
    instrument,
};

use crate::{
    domain::{
        Contact,
        ContactState,
        Hasher,
        NodeId,
        Path,
        PathSegmentation,
        PathSegments,
        RoutingTable,
        SourceRoute,
        Timestamp,
        ULNState,
        ULNTable,
        UnderlayNeighborId,
        VICINITY_RADIUS,
        protocol_event::forwarding::{
            PathIdEntry,
            PathIdForwardingEntry,
        },
        protocol_message::{
            CommonHeader,
            Nonce,
            PathSetupReqData,
            PathSetupRspData,
            PathTeardownReqData,
            ProtocolMessage,
            ProtocolMessageKind,
            ReqRspMessage,
            WireFormatMessage,
        },
    },
    runtime::UseCaseRuntime,
    use_cases::{
        ContactEvent,
        EventHandler,
        HandlingResult,
        TimerId,
        UseCase,
        UseCaseContext,
        UseCaseEvent,
        UseCaseState,
    },
};

/// Configuration for [ExplicitPathManagement] use case.
#[derive(Debug)]
pub struct EPMConfig {
    /// Maximum time an entry of the PathId forwarding table requested by other nodes
    /// is kept without being refreshed.
    ///
    /// Should be three times the interval in which nodes refresh the paths to their contacts
    /// (see [PathProbingConfig::path_refresh_interval](super::path_probing::PathProbingConfig::path_refresh_interval)).
    pub max_age: Duration,
    /// Interval to perform the garbage collection of entries that were not refreshed.
    pub cleanup_interval: Duration,
    /// Hasher to use for derivation of [PathIDs](crate::domain::PathId) from [Path]s.
    pub hasher: Hasher,
    /// How paths to contacts are split into path segments.
    pub segmentation: PathSegmentation,
}

impl Default for EPMConfig {
    fn default() -> Self {
        Self {
            max_age: Duration::from_secs(90),
            cleanup_interval: Duration::from_secs(10),
            hasher: Hasher::default(),
            segmentation: PathSegmentation::default(),
        }
    }
}

/// Represents the explicit path management use case.
///
/// Paths to contacts are split into up to two [segments](PathSegments). Their PathIds are
/// precomputed by the nodes on the path unless the path is at least six hops long. For these
/// paths some nodes have to install entries in their PathId forwarding table explicitly:
///
/// - the nodes of the first segment that are more than two hops away from its end and
/// - the node at the end of the first segment, where the second segment starts.
///
/// This use case implements:
///
/// - PathSetup for contacts with such paths; the PathSetupRsp confirms that the path is
///   [established in the forwarding tier](Path::is_forwarding_established).
/// - PathTeardown for contacts that are removed from or invalidated in the routing table.
/// - Handling of traversing PathSetupReq, PathTeardownReq and ProbeReq messages. The entries
///   requested by other nodes are soft state in the
///   [PathIdFwdState](crate::domain::PathIdFwdState): every PathSetupReq and ProbeReq
///   traveling along a path refreshes the entries of this path in all nodes it passes
///   (and installs them if they are missing).
/// - Periodic garbage collection of entries that were not refreshed.
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
    C::UnderlayNeighborTable:
        ULNTable + Deref<Target = HashMap<NodeId, (UnderlayNeighborId, ULNState)>>,
{
    /// Returns if the active path of the contact requires an explicit path setup.
    fn requires_setup(&self, contact: &Contact) -> bool {
        contact.path().is_some_and(|path| {
            PathSegments::split(path, self.config.segmentation).requires_setup()
        })
    }

    fn send_setup_req(&mut self, context: &C, contact: &Contact) {
        // just in case the contact has been invalidated meanwhile
        let Some(path) = contact.path().filter(|_| contact.is_valid()) else {
            return;
        };
        let EPMState::Running { pending_setups, .. } = &mut self.state else {
            log::warn!(target: "explicit_path_management", "Tried to send PathSetupReq while not initialized");
            return;
        };

        let mut nonce = Nonce::random();
        while pending_setups.contains_key(&nonce) {
            nonce = Nonce::random();
        }
        pending_setups.insert(
            nonce,
            PendingSetup {
                contact: *contact.id(),
                path: path.clone(),
                sent: Timestamp::now(),
            },
        );

        let source_route = SourceRoute::new(*context.root_id(), path.clone());
        let message = ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::PathSetupReq,
                *context.root_id(),
                *source_route.destination(),
                Some(nonce),
                *context.uln_table().state_seq_nr(),
                context.uln_table().size(),
            ),
            data: PathSetupReqData,
            not_via: None,
            source_route,
        };
        context
            .runtime()
            .send_message(message, context.uln_table().deref(), context.root_id());
    }

    fn send_setup_rsp(&self, context: &C, req: ReqRspMessage<PathSetupReqData>) {
        let source = *req.source();
        let message = ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::PathSetupRsp,
                *context.root_id(),
                source,
                Some(req.msg_id()),
                *context.uln_table().state_seq_nr(),
                context.uln_table().size(),
            ),
            data: PathSetupRspData,
            not_via: None,
            source_route: SourceRoute::from_reversed(req.source_route),
        };
        context
            .runtime()
            .send_message(message, context.uln_table().deref(), context.root_id());

        log::trace!(target: "explicit_path_management", "Sent PathSetupRsp to {source}");
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
                *context.uln_table().state_seq_nr(),
                context.uln_table().size(),
            ),
            data: PathTeardownReqData,
            not_via: None,
            source_route,
        };
        context
            .runtime()
            .send_message(message, context.uln_table().deref(), context.root_id());
    }

    /// The PathSetupRsp confirms that the PathIds of the path are installed in the nodes
    /// on the path, so the forwarding tier is able to use it.
    fn confirm_setup(&mut self, context: &C, nonce: &Nonce) {
        let EPMState::Running { pending_setups, .. } = &mut self.state else {
            return;
        };
        let Some(PendingSetup { contact, path, .. }) = pending_setups.remove(nonce) else {
            return;
        };

        let mut rt = context.routing_table_mut();
        let Some(mut contact) = rt.contact_mut(&contact) else {
            log::trace!(target: "explicit_path_management", "Ignoring PathSetupRsp: contact {contact} doesn't exist anymore");
            return;
        };
        match contact.path_mut() {
            // meanwhile the contact may use another path
            Some(active_path) if active_path.is_same_path_as(&path) => {
                active_path.update_last_path_refresh(Timestamp::now());
            }
            _ => {
                log::trace!(target: "explicit_path_management", "Ignoring PathSetupRsp: path of contact {} changed", contact.id());
            }
        }
    }

    /// Removes the entries requested by other nodes that were not refreshed.
    fn perform_cleanup(&mut self, context: &C) {
        let updates = context.fwd_state_mut().garbage_collect(self.config.max_age);
        for update in updates {
            log::debug!(target: "explicit_path_management", "Entry was not refreshed: {update}");
            context.runtime().update_fwd_tables(update);
        }

        if let EPMState::Running { pending_setups, .. } = &mut self.state {
            // unanswered requests, the path probing takes care of these paths
            pending_setups
                .retain(|_, pending| pending.sent.to_age_duration() <= self.config.max_age);
        }
    }

    /// Derives the entry of the PathId forwarding table this node needs for the path of the
    /// source of the [SourceRoute] to its destination.
    ///
    /// Returns [None] if no entry is needed, i.e., if the node is the last one of the path,
    /// or if it can't be derived.
    fn path_id_entry(
        &self,
        context: &C,
        source_route: &SourceRoute,
    ) -> Option<(PathIdEntry, usize)> {
        let (path, index) = source_route.path_of_source()?;
        let in_path = PathSegments::split_ids(&path, self.config.segmentation).in_path_at(index)?;
        if in_path.first() != Some(context.root_id()) {
            log::warn!(target: "explicit_path_management", "Current hop of source route is not this node; Ignoring");
            return None;
        }
        // the last node of a path has its own entry for its NodeId
        let out_path = in_path.get(1..).filter(|out_path| !out_path.is_empty())?;

        let Some((next_hop, _)) = context.uln_table().get(&out_path[0]).cloned() else {
            log::error!(target: "explicit_path_management", "Path via unknown underlay neighbor {}; Ignoring", out_path[0]);
            return None;
        };

        Some((
            PathIdEntry::Forward(PathIdForwardingEntry {
                in_path_id: self.config.hasher.hash(in_path),
                out_path_id: self.config.hasher.hash(out_path),
                next_hop,
            }),
            in_path.len(),
        ))
    }

    /// Installs or refreshes the entry of the PathId forwarding table this node needs for
    /// the path of the source of the [SourceRoute] to its destination.
    ///
    /// The entry is usually precomputed unless the remaining path of the segment is longer
    /// than the vicinity radius. It is installed in any case, as the precomputation may not
    /// have happened yet. Precomputed entries don't depend on being refreshed.
    fn register_path(&mut self, context: &C, source_route: &SourceRoute) {
        let Some((entry, _)) = self.path_id_entry(context, source_route) else {
            return;
        };
        let update = context.fwd_state_mut().add_foreign(entry);
        if let Some(update) = update {
            log::debug!(target: "explicit_path_management", "Installing entry requested by {}: {update}", source_route.source());
            context.runtime().update_fwd_tables(update);
        }
    }

    /// The source of the [SourceRoute] doesn't need the entry of the PathId forwarding table
    /// for its path anymore.
    ///
    /// Other nodes may use the same entry unknowingly, so it is not removed here but left to
    /// the garbage collection unless it gets refreshed.
    fn unregister_path(&mut self, context: &C, source_route: &SourceRoute) {
        let Some((entry, in_path_size)) = self.path_id_entry(context, source_route) else {
            return;
        };
        // precomputed entries are not affected
        if in_path_size > VICINITY_RADIUS {
            context.fwd_state_mut().unset_foreign(entry.in_path_id());
        }
    }

    fn create_new_cleanup_timer(&mut self, context: &C) {
        if let EPMState::Running { cleanup_timer, .. } = &mut self.state {
            *cleanup_timer = context
                .runtime()
                .register_timer(self.config.cleanup_interval);
        }
    }
}

impl<C, const BUCKET_SIZE: usize> EventHandler for ExplicitPathManagement<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable:
        ULNTable + Deref<Target = HashMap<NodeId, (UnderlayNeighborId, ULNState)>>,
{
    type Context = C;
    type Error = EPMError;
    type Value = HandlingResult;

    #[instrument(
        // the state is formatted for every event: only when tracing
        level = Level::TRACE,
        target = "explicit_path_management",
        "explicit_path_management",
        skip(self, context, event),
        fields(
            state = ?self.state,
            config = ?self.config
        )
        ret(level = Level::TRACE),
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
                    ContactEvent::New(contact) if self.requires_setup(&contact) => {
                        self.send_setup_req(context, &contact);
                    }
                    ContactEvent::Updated { new, old } => {
                        match (
                            new.state(),
                            old.state(),
                            self.requires_setup(&new),
                            self.requires_setup(&old),
                        ) {
                            (ContactState::Valid, ContactState::Invalid(_), true, _)
                            | (ContactState::Valid, ContactState::Rediscovering(_), true, _) => {
                                // Contacts becomes valid
                                self.send_setup_req(context, &new);
                            }
                            (ContactState::Valid, ContactState::Valid, true, false) => {
                                // Contacts path changes to one that requires a setup
                                self.send_setup_req(context, &new);
                            }
                            (ContactState::Valid, ContactState::Valid, true, true)
                                if !same_path(&new, &old) =>
                            {
                                // path changes between paths that require a setup
                                self.send_setup_req(context, &new);
                                self.send_teardown_req(context, &old);
                            }
                            (ContactState::Valid, &ContactState::Valid, false, true) => {
                                // Contacts path changed to one that doesn't require a setup
                                self.send_teardown_req(context, &old);
                            }
                            (
                                ContactState::Invalid(_),
                                ContactState::Valid,
                                new_requires_setup,
                                old_requires_setup,
                            ) => {
                                // Contact gets invalid
                                if new_requires_setup {
                                    self.send_teardown_req(context, &old);
                                }
                                if old_requires_setup && !same_path(&new, &old) {
                                    self.send_teardown_req(context, &new);
                                }
                            }
                            _ => {
                                // Ignored Cases:
                                // 1. Contact changes while being invalid
                                // 2. Contact changes between paths that don't require a setup
                                // 3. Contact changes without path change
                                // 4. Contact gets valid with a path that doesn't require a setup
                            }
                        }
                    }
                    ContactEvent::Removed(contact) if self.requires_setup(&contact) => {
                        self.send_teardown_req(context, &contact);
                    }
                    _ => {}
                }
            }
            // ========== Timers ==========
            (UseCaseEvent::Timer(timer), EPMState::Running { cleanup_timer, .. })
                if &timer == cleanup_timer =>
            {
                self.perform_cleanup(context);
                self.create_new_cleanup_timer(context);
            }
            // ========== Protocol Messages ==========
            (
                UseCaseEvent::Message(ProtocolMessage::PathSetupReq(req), _),
                EPMState::Running { .. },
            ) => {
                let unprocessed_hops = req.source_route.remaining_path().size(); // X, [A], B, C, D -> 4

                // process current hop
                self.register_path(context, &req.source_route);
                let processed_hops = unprocessed_hops.saturating_sub(1);

                // The following nodes have the PathIds of the remaining path precomputed
                // => the setup is complete, confirm it to the requester
                if processed_hops <= VICINITY_RADIUS {
                    log::trace!(
                        target: "explicit_path_management",
                        "PathSetupReq complete, the remaining path is precomputed: {req:?}"
                    );
                    self.send_setup_rsp(context, req);
                    return Ok(HandlingResult::Handled);
                }
            }
            (
                UseCaseEvent::Message(ProtocolMessage::PathSetupRsp(rsp), _),
                EPMState::Running { pending_setups, .. },
            ) if rsp.destination() == context.root_id()
                && pending_setups.contains_key(&rsp.msg_id()) =>
            {
                self.confirm_setup(context, &rsp.msg_id());
            }
            (
                UseCaseEvent::Message(ProtocolMessage::PathTeardownReq(req), _),
                EPMState::Running { .. },
            ) => {
                let unprocessed_hops = req.source_route.remaining_path().size();

                // process current hop
                self.unregister_path(context, &req.source_route);
                let processed_hops = unprocessed_hops.saturating_sub(1);

                // the remaining path is precomputed by the following nodes
                if processed_hops <= VICINITY_RADIUS {
                    log::trace!(
                        target: "explicit_path_management",
                        "Stop forwarding PathTeardownReq, the remaining path is precomputed: {req:?}"
                    );
                    return Ok(HandlingResult::Handled);
                }
            }
            (
                UseCaseEvent::Message(ProtocolMessage::ProbeReq(req), _),
                EPMState::Running { .. },
            ) => {
                if !req.exact() {
                    tracing::warn!(target: "explicit_path_management", "Received ProbeReq without exact flag");
                }

                // A ProbeReq travels the whole path and thereby refreshes the entries of the
                // path in all nodes on it. This also post installs missing entries.
                self.register_path(context, &req.source_route)
            }
            _ => {}
        }

        Ok(HandlingResult::NotHandled)
    }
}

/// Returns if both contacts use the same active path.
fn same_path(a: &Contact, b: &Contact) -> bool {
    match (a.path(), b.path()) {
        (Some(a), Some(b)) => a.is_same_path_as(b),
        (None, None) => true,
        _ => false,
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase for ExplicitPathManagement<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::UnderlayNeighborTable:
        ULNTable + Deref<Target = HashMap<NodeId, (UnderlayNeighborId, ULNState)>>,
{
    type State = EPMState;

    fn start(&mut self, context: &C) -> Result<(), Self::Error> {
        if self.state != EPMState::Initialized {
            return Err(EPMError::AlreadyStarted);
        }

        let cleanup_timer_id = context
            .runtime()
            .register_timer(self.config.cleanup_interval);
        self.state = EPMState::Running {
            cleanup_timer: cleanup_timer_id,
            pending_setups: HashMap::default(),
        };

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

/// A PathSetupReq waiting for its PathSetupRsp.
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct PendingSetup {
    /// Contact the path is set up for.
    pub contact: NodeId,
    /// Path that is set up.
    pub path: Path,
    /// When the PathSetupReq was sent.
    pub sent: Timestamp,
}

#[derive(Debug, Eq, PartialEq, Clone, Default)]
pub enum EPMState {
    /// The [UseCase] is waiting for startup.
    #[default]
    Initialized,
    /// The [UseCase] is running and has periodic garbage collection.
    Running {
        cleanup_timer: TimerId,
        /// PathSetupReqs waiting for their PathSetupRsp by message id.
        pending_setups: HashMap<Nonce, PendingSetup>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Output,
        context::{
            ContextConfig,
            SyncContext,
        },
        domain::{
            ConnectionId,
            ForwardingTablesUpdate,
            InterfaceId,
            PathId,
            PathIdTableUpdate,
            SafeStateSeqNr,
            UnderlayNeighborSource,
            fwd_state::FwdEntryFlags,
            protocol_message::ProbeReqData,
            routing_table::SingleBucketRT,
            underlay_neighbor_table::InMemoryULNTable,
        },
        runtime::testing::TestingUseCaseRuntime,
        tests::TestClock,
    };

    const BUCKET_SIZE: usize = 20;
    type Ctx =
        SyncContext<SingleBucketRT<BUCKET_SIZE>, TestingUseCaseRuntime, (), InMemoryULNTable, ()>;
    type Epm = ExplicitPathManagement<Ctx, BUCKET_SIZE>;

    /// Figure 3 of the draft: X -> A Q M Z C E T
    const X: usize = 0;
    const A: usize = 1;
    const Q: usize = 2;
    const M: usize = 3;
    const Z: usize = 4;
    const C: usize = 5;
    const T: usize = 7;

    fn node(index: usize) -> NodeId {
        NodeId::with_msb(index as u8 + 1)
    }
    /// The path of [X] to its contact [T].
    fn path_to_t() -> Path {
        Path::try_from((A..=T).map(node).collect::<Vec<_>>()).unwrap()
    }
    fn ulnid() -> UnderlayNeighborId {
        UnderlayNeighborId {
            interface_id: InterfaceId::try_from(1).unwrap(),
            connection_id: ConnectionId(1),
        }
    }
    fn path_id(nodes: std::ops::RangeInclusive<usize>) -> PathId {
        Hasher::default().hash(&nodes.map(node).collect::<Vec<_>>())
    }
    /// The use case running on the node with the given index,
    /// which has the next node of the path as underlay neighbor.
    fn setup(index: usize) -> (Epm, Ctx) {
        crate::tests::init();

        let ctx = Ctx::new(ContextConfig {
            root_id: node(index),
            routing_table: SingleBucketRT::<BUCKET_SIZE>::new(node(index)),
            runtime: TestingUseCaseRuntime::with_unique_timer_ids(),
            insertion_strategy: (),
            uln_table: InMemoryULNTable::new(),
            vicinity_graph: (),
        });
        if index < T {
            ctx.uln_table_mut().insert(node(index + 1), ulnid());
        }
        if index > X {
            ctx.uln_table_mut().insert(node(index - 1), ulnid());
        }

        let mut epm = Epm::new(EPMConfig::default());
        epm.start(&ctx).unwrap();
        (epm, ctx)
    }

    /// The source route of a message of [X] to [T] arriving at the node with the given index.
    fn route_at(index: usize) -> SourceRoute {
        let mut route = SourceRoute::new(node(X), path_to_t());
        for _ in A..index {
            route.advance();
        }
        assert_eq!(route.current_hop(), &node(index));
        route
    }

    fn header(kind: ProtocolMessageKind, nonce: u64) -> CommonHeader {
        CommonHeader::new(kind, node(X), node(T), Some(nonce.into()), 1, 1)
    }

    fn setup_req(index: usize) -> ProtocolMessage {
        ReqRspMessage {
            common_header: header(ProtocolMessageKind::PathSetupReq, 42),
            data: PathSetupReqData,
            not_via: None,
            source_route: route_at(index),
        }
        .into()
    }

    fn teardown_req(index: usize) -> ProtocolMessage {
        ReqRspMessage {
            common_header: header(ProtocolMessageKind::PathTeardownReq, 43),
            data: PathTeardownReqData,
            not_via: None,
            source_route: route_at(index),
        }
        .into()
    }

    fn probe_req(index: usize) -> ProtocolMessage {
        ReqRspMessage {
            common_header: header(ProtocolMessageKind::ProbeReq, 44),
            data: ProbeReqData,
            not_via: None,
            source_route: route_at(index),
        }
        .into()
    }

    fn handle(epm: &mut Epm, ctx: &Ctx, message: ProtocolMessage) -> HandlingResult {
        epm.handle_event(
            ctx,
            UseCaseEvent::Message(message, UnderlayNeighborSource::UnderlayNeighbor(ulnid())),
        )
        .unwrap()
    }

    /// Lets the garbage collection run.
    fn cleanup(epm: &mut Epm, ctx: &Ctx) {
        let EPMState::Running { cleanup_timer, .. } = epm.state() else {
            panic!("not running");
        };
        epm.handle_event(ctx, UseCaseEvent::Timer(*cleanup_timer))
            .unwrap();
    }

    /// Entries installed and incoming PathIds removed since the last call
    /// and the messages sent.
    fn output(ctx: &Ctx) -> (Vec<PathIdEntry>, Vec<PathId>, Vec<ProtocolMessage>) {
        let (mut installed, mut removed, mut sent) = (Vec::new(), Vec::new(), Vec::new());
        for output in ctx.runtime().output() {
            match output {
                Output::UpdateForwardingTables(ForwardingTablesUpdate::PathIdTableUpdate(
                    PathIdTableUpdate::CreateOrUpdate(entry),
                )) => installed.push(entry),
                Output::UpdateForwardingTables(ForwardingTablesUpdate::PathIdTableUpdate(
                    PathIdTableUpdate::Remove(in_path_id),
                )) => removed.push(in_path_id),
                Output::SendProtocolMessage(message, _) => sent.push(message),
                other => panic!("unexpected output {other:?}"),
            }
        }
        (installed, removed, sent)
    }

    fn forward(in_path: std::ops::RangeInclusive<usize>) -> PathIdEntry {
        PathIdEntry::Forward(PathIdForwardingEntry {
            in_path_id: path_id(in_path.clone()),
            out_path_id: path_id(*in_path.start() + 1..=*in_path.end()),
            next_hop: ulnid(),
        })
    }

    #[test]
    fn first_node_installs_first_segment() {
        let _clock = TestClock::new();
        let (mut epm, ctx) = setup(A);

        // H(A|Q|M|Z) -> H(Q|M|Z) is not precomputed by A
        assert!(matches!(
            handle(&mut epm, &ctx, setup_req(A)),
            HandlingResult::NotHandled
        ));
        let (installed, removed, sent) = output(&ctx);
        assert_eq!(installed, [forward(A..=Z)]);
        assert!(removed.is_empty());
        assert!(sent.is_empty(), "forwarded by ForwardProtocolMessage");
        assert_eq!(
            ctx.fwd_state().get(&path_id(A..=Z)).unwrap().flags(),
            FwdEntryFlags::FOREIGN
        );
    }

    #[test]
    fn nodes_with_precomputed_entries_install_nothing() {
        let _clock = TestClock::new();
        for index in [Q, M] {
            let (mut epm, ctx) = setup(index);
            // precomputed for the vicinity: H(Q|M|Z) and H(M|Z)
            let update = ctx.fwd_state_mut().add_precalc(forward(index..=Z));
            assert!(update.is_some());

            assert!(matches!(
                handle(&mut epm, &ctx, setup_req(index)),
                HandlingResult::NotHandled
            ));
            let (installed, removed, sent) = output(&ctx);
            assert!(installed.is_empty() && removed.is_empty() && sent.is_empty());

            let fwd_state = ctx.fwd_state();
            let entry = fwd_state.get(&path_id(index..=Z)).unwrap();
            assert_eq!(entry.flags(), FwdEntryFlags::PRECALC);
            assert!(entry.is_used());
        }
    }

    #[test]
    fn missing_precomputed_entry_is_installed() {
        let _clock = TestClock::new();
        let (mut epm, ctx) = setup(Q);

        // e.g. the adjacency to M just came up and the precomputation didn't happen yet
        handle(&mut epm, &ctx, setup_req(Q));
        assert_eq!(output(&ctx).0, [forward(Q..=Z)]);
    }

    #[test]
    fn start_of_second_segment_installs_it_and_answers() {
        let _clock = TestClock::new();
        let (mut epm, ctx) = setup(Z);

        // H(Z|C|E|T) -> H(C|E|T) is not precomputed by Z,
        // the following nodes have the PathIds of the remaining path precomputed
        assert!(matches!(
            handle(&mut epm, &ctx, setup_req(Z)),
            HandlingResult::Handled
        ));
        let (installed, _, sent) = output(&ctx);
        assert_eq!(installed, [forward(Z..=T)]);

        let [ProtocolMessage::PathSetupRsp(rsp)] = sent.as_slice() else {
            panic!("expected PathSetupRsp, got {sent:?}");
        };
        assert_eq!(rsp.msg_id(), 42.into());
        assert_eq!(rsp.destination(), &node(X));
        assert_eq!(rsp.source_route.source(), &node(Z));
        assert_eq!(rsp.source_route.current_hop(), &node(M));
    }

    #[test]
    fn foreign_entry_expires_unless_refreshed_by_probes() {
        let mut clock = TestClock::new();
        let (mut epm, ctx) = setup(A);
        let max_age = EPMConfig::default().max_age;
        handle(&mut epm, &ctx, setup_req(A));
        let _ = output(&ctx);

        clock.advance(max_age);
        handle(&mut epm, &ctx, probe_req(A));
        clock.advance(max_age);
        cleanup(&mut epm, &ctx);
        let (installed, removed, _) = output(&ctx);
        assert!(installed.is_empty() && removed.is_empty(), "refreshed");

        clock.advance(Duration::from_secs(1));
        cleanup(&mut epm, &ctx);
        assert_eq!(output(&ctx).1, [path_id(A..=Z)]);
        assert!(ctx.fwd_state().is_empty());
    }

    #[test]
    fn probe_installs_missing_entry() {
        let _clock = TestClock::new();
        let (mut epm, ctx) = setup(Z);

        // e.g. Z lost its state by a restart
        assert!(matches!(
            handle(&mut epm, &ctx, probe_req(Z)),
            HandlingResult::NotHandled
        ));
        let (installed, _, sent) = output(&ctx);
        assert_eq!(installed, [forward(Z..=T)]);
        assert!(sent.is_empty());
    }

    #[test]
    fn destination_needs_no_entry() {
        let _clock = TestClock::new();
        let (mut epm, ctx) = setup(T);

        handle(&mut epm, &ctx, probe_req(T));
        let (installed, removed, sent) = output(&ctx);
        assert!(installed.is_empty() && removed.is_empty() && sent.is_empty());
    }

    #[test]
    fn teardown_lets_entry_expire() {
        let mut clock = TestClock::new();
        let (mut epm, ctx) = setup(A);
        handle(&mut epm, &ctx, setup_req(A));
        let _ = output(&ctx);

        assert!(matches!(
            handle(&mut epm, &ctx, teardown_req(A)),
            HandlingResult::NotHandled
        ));
        // other nodes may still use the entry
        assert!(ctx.fwd_state().contains(&path_id(A..=Z)));
        assert!(output(&ctx).1.is_empty());

        clock.advance(EPMConfig::default().max_age + Duration::from_secs(1));
        cleanup(&mut epm, &ctx);
        assert_eq!(output(&ctx).1, [path_id(A..=Z)]);
    }

    #[test]
    fn teardown_stops_at_start_of_second_segment() {
        let _clock = TestClock::new();
        let (mut epm, ctx) = setup(Z);
        assert!(matches!(
            handle(&mut epm, &ctx, teardown_req(Z)),
            HandlingResult::Handled
        ));
    }

    #[test]
    fn teardown_keeps_precomputed_entry() {
        let _clock = TestClock::new();
        let (mut epm, ctx) = setup(C);
        ctx.fwd_state_mut().add_precalc(forward(C..=T));

        epm.handle_event(
            &ctx,
            UseCaseEvent::Message(teardown_req(C), UnderlayNeighborSource::Local),
        )
        .unwrap();
        assert_eq!(
            ctx.fwd_state().get(&path_id(C..=T)).unwrap().flags(),
            FwdEntryFlags::PRECALC
        );
    }

    /// [X] with the contact [T], returns the sent PathSetupReq.
    fn setup_with_contact() -> (Epm, Ctx, ReqRspMessage<PathSetupReqData>) {
        let (mut epm, ctx) = setup(X);
        let contact = Contact::new(path_to_t(), SafeStateSeqNr::try_from(1).unwrap());
        ctx.routing_table_mut().add(contact.clone()).unwrap();
        epm.handle_event(
            &ctx,
            UseCaseEvent::Contact(Box::new(ContactEvent::New(contact))),
        )
        .unwrap();

        let (_, _, mut sent) = output(&ctx);
        let Some(ProtocolMessage::PathSetupReq(req)) = sent.pop() else {
            panic!("expected PathSetupReq");
        };
        assert!(sent.is_empty());
        (epm, ctx, req)
    }

    fn setup_rsp(req: &ReqRspMessage<PathSetupReqData>) -> ProtocolMessage {
        // answered by Z, traveled back to X
        let mut route = SourceRoute::from_reversed(route_at(Z));
        while !route.is_finished() {
            route.advance();
        }
        ReqRspMessage {
            common_header: CommonHeader::new(
                ProtocolMessageKind::PathSetupRsp,
                node(Z),
                node(X),
                Some(req.msg_id()),
                1,
                1,
            ),
            data: PathSetupRspData,
            not_via: None,
            source_route: route,
        }
        .into()
    }

    #[test]
    fn setup_rsp_establishes_path_of_contact() {
        let _clock = TestClock::new();
        let (mut epm, ctx, req) = setup_with_contact();
        assert_eq!(req.source_route.destination(), &node(T));
        let established = |ctx: &Ctx| {
            ctx.routing_table()
                .contact(&node(T))
                .unwrap()
                .path()
                .unwrap()
                .is_forwarding_established(PathSegmentation::TwoSegments)
        };
        assert!(!established(&ctx));

        handle(&mut epm, &ctx, setup_rsp(&req));
        assert!(established(&ctx));
    }

    #[test]
    fn setup_rsp_for_changed_path_is_ignored() {
        let _clock = TestClock::new();
        let (mut epm, ctx, req) = setup_with_contact();

        // meanwhile the contact uses another path that requires a setup
        let other_path = Path::try_from([A, Q, M, Z, C, 8, T].map(node).as_slice()).unwrap();
        ctx.routing_table_mut()
            .contact_mut(&node(T))
            .unwrap()
            .set_path(other_path);

        handle(&mut epm, &ctx, setup_rsp(&req));
        assert!(
            !ctx.routing_table()
                .contact(&node(T))
                .unwrap()
                .path()
                .unwrap()
                .is_forwarding_established(PathSegmentation::TwoSegments)
        );
    }

    #[test]
    fn short_paths_need_no_setup() {
        let _clock = TestClock::new();
        let (mut epm, ctx) = setup(X);
        // five hops
        let path = Path::try_from((A..=C).map(node).collect::<Vec<_>>()).unwrap();
        let contact = Contact::new(path, SafeStateSeqNr::try_from(1).unwrap());

        epm.handle_event(
            &ctx,
            UseCaseEvent::Contact(Box::new(ContactEvent::New(contact))),
        )
        .unwrap();
        assert!(output(&ctx).2.is_empty());
    }
}
