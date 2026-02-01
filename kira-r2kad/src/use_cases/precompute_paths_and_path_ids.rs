use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::marker::PhantomData;
use std::ops::Deref;
use std::time::Duration;
use tracing::{Level, instrument};

use crate::domain::protocol_event::forwarding::{
    PathIdEntry, PathIdForwardingEntry, PathIdTableUpdate,
};
use crate::domain::{
    Hasher, NodeId, RoutingTable, UnderlayNeighborId, VICINITY_RADIUS, VicinityGraph,
};

use crate::use_cases::{
    EventHandler, NeverError, TimerId, UseCase, UseCaseContext, UseCaseEvent, UseCaseRuntime,
    UseCaseState, VicinityEvent,
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
    C::UnderlayNeighborTable: Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
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

            let Some(ulnid) = context.uln_table().get(out_path.first()).cloned() else {
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
}

impl<C, const BUCKET_SIZE: usize> EventHandler for PrecomputePathIds<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    C::UnderlayNeighborTable: Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
    C::VicinityGraph: VicinityGraph + Debug,
{
    type Context = C;
    type Error = NeverError;
    type Value = ();

    #[instrument(
        level = Level::TRACE,
        target = "precompute_paths_and_path_ids",
        "precompute_paths_and_path_ids",
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
    C::UnderlayNeighborTable: Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
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
