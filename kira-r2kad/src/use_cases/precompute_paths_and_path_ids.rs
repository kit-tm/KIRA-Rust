use std::{
    collections::{
        HashMap,
        HashSet,
    },
    fmt::Debug,
    marker::PhantomData,
    ops::Deref,
    time::Duration,
};

use tracing::{
    Level,
    instrument,
};

use crate::{
    domain::{
        Hasher,
        NodeId,
        RoutingTable,
        ULNState,
        UnderlayNeighborId,
        UnderlayNeighborUpdate,
        VICINITY_RADIUS,
        VicinityGraph,
        protocol_event::forwarding::{
            PathIdEntry,
            PathIdForwardingEntry,
            PathIdTableUpdate,
        },
    },
    use_cases::{
        EventHandler,
        NeverError,
        TimerId,
        UseCase,
        UseCaseContext,
        UseCaseEvent,
        UseCaseRuntime,
        UseCaseState,
        VicinityEvent,
    },
};

/// Configuration for [UseCase] [PrecomputePathIds].
#[derive(Debug, Default)]
pub struct PrecomputePathIdsConfig {
    /// Interval in which the precomputation will take place.
    ///
    /// If [None] is passed the precomputation will happen on every change.
    pub update_interval: Option<Duration>,
    /// Hasher to use for generation of [PathIds] from [Paths].
    ///
    /// [PathIds]: crate::domain::PathId
    /// [Paths]: crate::domain::Path
    pub hasher: Hasher,
}

/// [UseCase] implementation representing the Precomputation of [Paths] and
/// [PathIds] for all nodes in a configurable vicinity.
///
/// This also removes all entries it generates if they're not valid anymore.
///
/// Based on its [PrecomputePathIdsConfig] the precomputation happens on every change or in a
/// periodic interval.
///
/// [PathIds]: crate::domain::PathId
/// [Paths]: crate::domain::Path
#[derive(Debug)]
pub struct PrecomputePathIds<C, const BUCKET_SIZE: usize> {
    _pd: PhantomData<C>,
    state: PrecomputeState,
    config: PrecomputePathIdsConfig,
    old_entries: HashSet<PathIdEntry>,
}

impl<C, const BUCKET_SIZE: usize> PrecomputePathIds<C, BUCKET_SIZE> {
    pub fn new(config: PrecomputePathIdsConfig) -> Self {
        Self {
            _pd: PhantomData,
            state: PrecomputeState::default(),
            config,
            old_entries: HashSet::new(),
        }
    }
}

impl<C, const BUCKET_SIZE: usize> Default for PrecomputePathIds<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self::new(Default::default())
    }
}

impl<C, const BUCKET_SIZE: usize> PrecomputePathIds<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    C::UnderlayNeighborTable: Deref<Target = HashMap<NodeId, (UnderlayNeighborId, ULNState)>>,
    C::VicinityGraph: VicinityGraph + Debug,
{
    #[instrument(
        level = Level::TRACE,
        target = "precompute_paths_and_path_ids",
        skip(self, context),
        fields(?graph),
        ret
    )]
    fn gen_entries_from_graph(
        &self,
        context: &C,
        graph: &C::VicinityGraph,
    ) -> HashSet<PathIdEntry> {
        let mut entries = HashSet::new();
        for in_path in graph.vicinity_paths() {
            debug_assert!(
                in_path.size() <= VICINITY_RADIUS,
                "VicinityGraph should only generate Paths inside the Vicinity Radius"
            );
            debug_assert_eq!(
                in_path.first(),
                context.root_id(),
                "in_path does not contain own NodeID as first element"
            );

            let Ok(out_path) = in_path.clone().into_iter().skip(1).collect() else {
                tracing::warn!(target: "precompute_paths_and_path_ids", "VicinityGraph generated path of size 1");
                continue;
            };

            let Some((ulnid, _)) = context.uln_table().get(out_path.first()).cloned() else {
                tracing::warn!(target: "precompute_paths_and_path_ids", underlay_neighbor = ?out_path.first(), "VicinityGraph generated path over invalid underlay neighbor");
                continue;
            };

            let entry = PathIdEntry::Forward(PathIdForwardingEntry {
                in_path_id: self.config.hasher.hash(&in_path),
                out_path_id: self.config.hasher.hash(&out_path),
                next_hop: ulnid,
            });
            entries.insert(entry);
        }
        entries
    }

    #[instrument(
        level = Level::TRACE,
        target = "precompute_paths_and_path_ids",
        skip_all,
    )]
    fn precompute_paths_and_ids(&mut self, context: &C) {
        let graph_entries = self.gen_entries_from_graph(context, &context.vicinity_graph());
        let new_entries = graph_entries.difference(&self.old_entries);
        let deleted_entries = self.old_entries.difference(&graph_entries);

        tracing::debug!(target: "precompute_paths_and_path_ids", ?new_entries, ?deleted_entries, "changes of paths in the vicinity");
        for deleted_entry in deleted_entries {
            context
                .runtime()
                .update_fwd_tables(PathIdTableUpdate::Remove(
                    deleted_entry.in_path_id().clone(),
                ));
        }
        for new_entry in new_entries {
            context
                .runtime()
                .update_fwd_tables(PathIdTableUpdate::CreateOrUpdate(new_entry.clone()));
        }

        self.old_entries = graph_entries;
        context.vicinity_graph_mut().vicinity_processed();
    }

    /// Removes all entries that forward via the lost underlay neighbor.
    ///
    /// The kernel drops routes over a vanished link, so these entries must be
    /// forgotten here as well. Otherwise they would not be recognized as new
    /// (and thus not be reinstalled) when the neighbor comes back with the
    /// same [UnderlayNeighborId] and the vicinity graph looks as before.
    fn purge_entries_via(&mut self, context: &C, ulnid: &UnderlayNeighborId) {
        self.old_entries.retain(|entry| {
            let PathIdEntry::Forward(PathIdForwardingEntry { next_hop, .. }) = entry else {
                return true;
            };
            if next_hop != ulnid {
                return true;
            }
            tracing::debug!(target: "precompute_paths_and_path_ids", %entry, %ulnid, "removing entry via lost underlay neighbor");
            context
                .runtime()
                .update_fwd_tables(PathIdTableUpdate::Remove(entry.in_path_id().clone()));
            false
        });
    }
}

impl<C, const BUCKET_SIZE: usize> EventHandler for PrecomputePathIds<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    C::UnderlayNeighborTable: Deref<Target = HashMap<NodeId, (UnderlayNeighborId, ULNState)>>,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::VicinityGraph: VicinityGraph + Debug,
{
    type Context = C;
    type Error = NeverError;
    type Value = ();

    #[instrument(
        target = "precompute_paths_and_path_ids",
        "precompute_paths_and_path_ids",
        skip(self, context, event),
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
            UseCaseEvent::Vicinity(VicinityEvent::Changed) => {
                tracing::trace!(target: "precompute_paths_and_path_ids", "Vicinity Changed");

                if self.config.update_interval.is_none() {
                    self.precompute_paths_and_ids(context);
                }
            }
            UseCaseEvent::Timer(timer_id) => {
                if let PrecomputeState::Waiting {
                    resync_timer: own_id,
                    ..
                } = self.state
                {
                    if timer_id == own_id {
                        self.precompute_paths_and_ids(context);
                    }
                    return Ok(());
                }
            }
            UseCaseEvent::UnderlayUpdate(UnderlayNeighborUpdate::UnderlayNeighborDown(ulnid)) => {
                self.purge_entries_via(context, &ulnid);
            }
            // FIXME: react on external changes of the vicinity graph
            _ => {}
        }

        Ok(())
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase for PrecomputePathIds<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    C::UnderlayNeighborTable: Deref<Target = HashMap<NodeId, (UnderlayNeighborId, ULNState)>>,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::VicinityGraph: VicinityGraph + Debug,
{
    type State = PrecomputeState;

    fn start(&mut self, context: &C) -> Result<(), Self::Error> {
        if let Some(duration) = self.config.update_interval {
            let timer_id = context.runtime().register_periodic_timer(duration);
            self.state = PrecomputeState::Waiting {
                resync_timer: timer_id,
            };
        }

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

#[derive(Debug, Eq, PartialEq, Clone, Default)]
pub enum PrecomputeState {
    #[default]
    Idle,
    Waiting {
        resync_timer: TimerId,
    },
    Error,
}

impl UseCaseState for PrecomputeState {
    fn is_error(&self) -> bool {
        self == &Self::Error
    }
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
            SafeStateSeqNr,
            ULNTable,
            routing_table::SingleBucketRT,
            underlay_neighbor_table::InMemoryULNTable,
            vicinity::vicinity_graph::PetVicinityGraph,
        },
        runtime::testing::TestingUseCaseRuntime,
    };

    const BUCKET_SIZE: usize = 20;
    type Ctx = SyncContext<
        SingleBucketRT<BUCKET_SIZE>,
        TestingUseCaseRuntime,
        (),
        InMemoryULNTable,
        PetVicinityGraph,
    >;
    type PP = PrecomputePathIds<Ctx, BUCKET_SIZE>;

    fn root() -> NodeId {
        NodeId::with_msb(1)
    }
    /// underlay neighbor that stays reachable
    fn a() -> NodeId {
        NodeId::with_msb(2)
    }
    /// underlay neighbor whose link fails
    fn b() -> NodeId {
        NodeId::with_msb(3)
    }
    /// two-hop neighbor behind [b]
    fn x() -> NodeId {
        NodeId::with_msb(4)
    }
    fn ulnid(id: u32) -> UnderlayNeighborId {
        UnderlayNeighborId {
            interface_id: InterfaceId::try_from(id).unwrap(),
            connection_id: ConnectionId(id),
        }
    }
    fn ssn() -> SafeStateSeqNr {
        SafeStateSeqNr::try_from(1).unwrap()
    }
    fn path_id(nodes: &[NodeId]) -> PathId {
        Hasher::default().hash(nodes)
    }

    /// Vicinity root–[a] (ulnid 1), root–[b] (ulnid 2) and [b]–[x];
    /// precomputation on every change.
    fn setup() -> (PP, Ctx) {
        crate::tests::init();

        let ctx = Ctx::new(ContextConfig {
            root_id: root(),
            routing_table: SingleBucketRT::<BUCKET_SIZE>::new(root()),
            runtime: TestingUseCaseRuntime::with_unique_timer_ids(),
            insertion_strategy: (),
            uln_table: InMemoryULNTable::new(),
            vicinity_graph: PetVicinityGraph::new(root()),
        });
        ctx.uln_table_mut().insert(a(), ulnid(1));
        ctx.uln_table_mut().insert(b(), ulnid(2));
        {
            let mut vg = ctx.vicinity_graph_mut();
            vg.insert(a(), &root(), ssn()).unwrap();
            vg.insert(b(), &root(), ssn()).unwrap();
            vg.insert(x(), &b(), ssn()).unwrap();
        }

        let mut pp = PP::new(PrecomputePathIdsConfig::default());
        pp.start(&ctx).unwrap();
        (pp, ctx)
    }

    fn handle(pp: &mut PP, ctx: &Ctx, event: UseCaseEvent) {
        pp.handle_event(ctx, event).unwrap();
    }

    /// in_path_ids of the installed and of the removed entries since the last call
    fn fwd_table_updates(ctx: &Ctx) -> (HashSet<PathId>, HashSet<PathId>) {
        let mut installed = HashSet::new();
        let mut removed = HashSet::new();
        for output in ctx.runtime().output() {
            match output {
                Output::UpdateForwardingTables(ForwardingTablesUpdate::PathIdTableUpdate(
                    PathIdTableUpdate::CreateOrUpdate(entry),
                )) => assert!(installed.insert(entry.in_path_id().clone())),
                Output::UpdateForwardingTables(ForwardingTablesUpdate::PathIdTableUpdate(
                    PathIdTableUpdate::Remove(in_path_id),
                )) => assert!(removed.insert(in_path_id)),
                other => panic!("unexpected output {other:?}"),
            }
        }
        (installed, removed)
    }

    fn all_entries() -> HashSet<PathId> {
        HashSet::from([
            path_id(&[root(), a()]),
            path_id(&[root(), b()]),
            path_id(&[root(), b(), x()]),
        ])
    }
    fn entries_via_b() -> HashSet<PathId> {
        HashSet::from([path_id(&[root(), b()]), path_id(&[root(), b(), x()])])
    }

    #[test]
    fn vicinity_change_installs_new_and_removes_vanished_entries() {
        let (mut pp, ctx) = setup();

        handle(
            &mut pp,
            &ctx,
            UseCaseEvent::Vicinity(VicinityEvent::Changed),
        );
        assert_eq!(fwd_table_updates(&ctx), (all_entries(), HashSet::new()));
        assert!(!ctx.vicinity_graph().vicinity_changed(), "change processed");

        // the link to b fails
        {
            let mut vg = ctx.vicinity_graph_mut();
            assert!(vg.remove_edge(&root(), &b()));
            let _ = vg.retain_vicinity().count();
        }
        ctx.uln_table_mut().remove(&b());
        handle(
            &mut pp,
            &ctx,
            UseCaseEvent::Vicinity(VicinityEvent::Changed),
        );

        assert_eq!(fwd_table_updates(&ctx), (HashSet::new(), entries_via_b()));
    }

    #[test]
    fn uln_down_purges_its_entries_so_they_are_reinstalled() {
        let (mut pp, ctx) = setup();
        handle(
            &mut pp,
            &ctx,
            UseCaseEvent::Vicinity(VicinityEvent::Changed),
        );
        let _ = fwd_table_updates(&ctx);

        handle(
            &mut pp,
            &ctx,
            UseCaseEvent::UnderlayUpdate(UnderlayNeighborUpdate::UnderlayNeighborDown(ulnid(2))),
        );
        assert_eq!(
            fwd_table_updates(&ctx),
            (HashSet::new(), entries_via_b()),
            "entries via the lost ULN are removed at once"
        );

        // b comes back with the same ulnid, the vicinity looks as before
        handle(
            &mut pp,
            &ctx,
            UseCaseEvent::Vicinity(VicinityEvent::Changed),
        );
        assert_eq!(fwd_table_updates(&ctx), (entries_via_b(), HashSet::new()));
    }
}
