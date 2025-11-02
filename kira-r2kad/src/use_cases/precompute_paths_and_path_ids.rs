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
    vicinity_graph,
};
use crate::messaging::{ProtocolMessage, RTableData, ReqRspMessage};
use crate::use_cases::{
    EventHandler, NeverError, TimerId, UseCase, UseCaseContext, UseCaseEvent, UseCaseRuntime,
    UseCaseState,
};

/// Configuration for [UseCase] [PrecomputePathIds].
#[derive(Debug, Default)]
pub struct PrecomputePathIdsConfig {
    /// Interval in which the precomputation will take place.
    ///
    /// If [None] is passed the precomputation will happen on every change.
    pub update_interval: Option<Duration>,
    /// Hasher to use for generation of [PathIds](crate::domain::PathId) from [Path]s.
    pub hasher: Hasher,
}

/// [UseCase] implementation representing the Precomputation of [Path]s and
/// [PathIds](crate::domain::PathId) for all nodes in a configurable vicinity.
///
/// This also removes all entries it generates if they're not valid anymore.
///
/// Based on its [PrecomputePathIdsConfig] the precomputation happens on every change or in a
/// periodic interval.
#[derive(Debug)]
pub struct PrecomputePathIds<C, const BUCKET_SIZE: usize> {
    _pd: PhantomData<C>,
    state: PrecomputeState,
    config: PrecomputePathIdsConfig,
    vicinity_changed: bool,
    old_entries: HashSet<PathIdEntry>,
}

impl<C, const BUCKET_SIZE: usize> PrecomputePathIds<C, BUCKET_SIZE> {
    pub fn new(config: PrecomputePathIdsConfig) -> Self {
        Self {
            _pd: PhantomData,
            state: PrecomputeState::default(),
            config,
            vicinity_changed: false,
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
                "VicinityGraph should only generate Paths inside the Vicinity-Radius"
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
        if !self.vicinity_changed {
            return;
        }

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
        self.vicinity_changed = false;
    }

    fn handle_vicinity_update(&mut self, context: &C, rtable_data: ReqRspMessage<RTableData>) {
        let hop_count = rtable_data.source_route.size() - 1; // excluding ourselves

        // Skip everything not inside vicinity radius
        if hop_count >= VICINITY_RADIUS {
            // shouldn't happen unless we falsely send requests outside our radius
            tracing::warn!(
                target: "precompute_paths_and_path_ids",
                source = %rtable_data.source(),
                source_route = ?rtable_data.source_route,
                "received vicinity information from outside the vicinity"
            );
            return;
        }

        let source = *rtable_data.source();
        let Some(source_ssn) = rtable_data.source_state_seq_nr.value() else {
            // shouldn't happen because the ForwardProtocolMessages
            // use-case would short circuit and blocks us from receiving the message
            tracing::warn!(
                target: "precompute_paths_and_path_ids",
                %source,
                ssn = %rtable_data.source_state_seq_nr,
                "underlay information updates by a node with an invalid state sequence number"
            );
            return;
        };

        let source_neighbors = rtable_data.data.contacts;
        tracing::trace!(
            target: "precompute_paths_and_path_ids",
            %source,
            neighbors = ?source_neighbors,
            "underlay information update"
        );

        let mut vicinity_graph = context.vicinity_graph_mut();

        let underlay_neighbor = hop_count == 1;
        let root_id = context.root_id();
        if underlay_neighbor {
            // underlay neighbors are only inserted after two way handshake so we do it here
            // 2 hop neighbors are added via one of our underlay neighbors rtable
            assert!(
                vicinity_graph.insert(source, root_id, source_ssn).is_ok(),
                "insertion of underlay neighbor into vicinity graph failed",
            );
            self.vicinity_changed = true;
        }

        // obtain vicinity graph entry
        let Some(source_vicinity_entry) = vicinity_graph.entry_mut(&source) else {
            if underlay_neighbor {
                panic!(
                    "vicinity graph entry of just inserted underlay neighbor can't be found: {source}"
                );
            }

            // shouldn't happen but we can cope with it by just doing nothing
            tracing::warn!(
                target: "precompute_paths_and_path_ids",
                node = %source,
                "vicinity graph entry not found",
            );
            return;
        };

        // update vicinity graph entry
        source_vicinity_entry.update_vicinity_ssn(source_ssn);
        source_vicinity_entry.update_last_seen(context.runtime().current_time());

        // inserting neighbors of source into our vicinity graph

        if !underlay_neighbor {
            // don't care about the source's neighbors on 2-hops because:
            // - link to 1-hop: synced via periodic and urgent ULNHellos
            // - link to another 2-hop: maybe interesting in the future
            // - link to > = 3 hop: completely useless
            return;
        }

        let mut old_source_neighbors: HashSet<_> = vicinity_graph.vicinity(&source).collect();
        for source_neighbor in source_neighbors.iter() {
            let nid = source_neighbor.id();
            if nid == root_id {
                old_source_neighbors.remove(nid);
                continue;
            }
            let observed_ssn = *source_neighbor.state_seq_nr();

            // only accept updates to us from our underlay neighbors
            if nid != root_id || underlay_neighbor {
                let old_observed_ssn = vicinity_graph
                    .entry(nid)
                    .map(vicinity_graph::Entry::observed_ssn);

                // don't decrease observed_ssn because we aren't directly talking to nid
                let observed_ssn = match old_observed_ssn {
                    Some(old_observed_ssn) if old_observed_ssn < &observed_ssn => {
                        tracing::trace!(
                            target: "precompute_paths_and_path_ids",
                            %source,
                            node = %nid,
                            %old_observed_ssn,
                            new_observed_ssn = %observed_ssn,
                            "observed higher state sequence number of vicinity node"
                        );
                        observed_ssn
                    }
                    None => {
                        tracing::debug!(
                            target: "precompute_paths_and_path_ids",
                            %source,
                            node = %nid,
                            %observed_ssn,
                            "insert new node into vicinity graph"
                        );
                        observed_ssn
                    }
                    Some(old_observed_ssn) => *old_observed_ssn, // old news
                };

                if let Err(err) = vicinity_graph.insert(*nid, &source, observed_ssn) {
                    tracing::warn!(
                        target: "precompute_paths_and_path_ids",
                        %source,
                        node = %nid,
                        err_msg = %err,
                        "inserting source neighbor into vicinity graph failed"
                    );
                    continue;
                };

                let new_link = old_source_neighbors.remove(nid);
                if new_link {
                    tracing::debug!(
                        target: "precompute_paths_and_path_ids",
                        %source,
                        node = %nid,
                        "added edge to vicinity graph"
                    );
                    self.vicinity_changed = true;
                }
            }
        }

        let removed_source_neighbors = old_source_neighbors;
        for removed_source_neighbor in removed_source_neighbors.iter() {
            assert!(
                vicinity_graph.remove_edge(&source, removed_source_neighbor),
                "removing edge of node's vicinity should change vicinity graph"
            );

            tracing::debug!(
                target: "precompute_paths_and_path_ids",
                %source,
                node = %removed_source_neighbor,
                "removed edge from vicinity graph"
            );
            self.vicinity_changed = true;
        }

        // clean up nodes moved outside the vicinity or got isolated
        // because of the removal of links
        if !removed_source_neighbors.is_empty() {
            let _removed_nodes = vicinity_graph.retain_vicinity();
            self.vicinity_changed = true;
        }
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
            UseCaseEvent::Message(ProtocolMessage::ULNDiscReq(rtable_data), _)
            | UseCaseEvent::Message(ProtocolMessage::ULNDiscRsp(rtable_data), _)
            | UseCaseEvent::Message(ProtocolMessage::QueryRouteRsp(rtable_data), _) => {
                self.handle_vicinity_update(context, rtable_data);

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
