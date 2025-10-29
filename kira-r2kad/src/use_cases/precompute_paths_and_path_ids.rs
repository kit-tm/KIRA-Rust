use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::marker::PhantomData;
use std::mem;
use std::ops::Deref;
use std::time::Duration;
use tracing::{Level, instrument};

use crate::domain::protocol_event::forwarding::{
    PathIdEntry, PathIdForwardingEntry, PathIdTableUpdate,
};
use crate::domain::{
    Contact, Hasher, NodeId, RoutingTable, UnderlayNeighborId, VICINITY_RADIUS, VicinityGraph,
};
use crate::messaging::ProtocolMessage;
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
                // Skip everything not in vicinity radius
                if rtable_data.source_route.size() >= VICINITY_RADIUS {
                    // shouldn't happen unless we falsely send requests outside our radius
                    tracing::warn!(
                        target: "precompute_paths_and_path_ids",
                        source=%rtable_data.source(),
                        source_route=?rtable_data.source_route,
                        "received vicinity information from outside the vicinity"
                    );

                    return Ok(());
                }

                let source = *rtable_data.source();
                let Some(source_ssn) = rtable_data.source_state_seq_nr.value() else {
                    // shouldn't happen because the ForwardProtocolMessages
                    // use-case would short circuit and blocks us from receiving the message
                    tracing::warn!(
                        target: "precompute_paths_and_path_ids",
                        %source,
                        ssn=%rtable_data.source_state_seq_nr,
                        "underlay information updates by a node with an invalid state sequence number"
                    );
                    return Ok(());
                };

                let underlay_contacts = rtable_data.data.contacts;
                tracing::trace!(target: "precompute_paths_and_path_ids",
                    %source,
                    neighbors=?underlay_contacts,
                    "underlay information update"
                );

                let mut vicinity_graph = context.vicinity_graph_mut();
                // TODO: prohibit link deletions to underlay neighbors on 2hops
                if let Err(err) = vicinity_graph.update_vicinity(
                    &source,
                    underlay_contacts.iter().map(Contact::id).copied(),
                    source_ssn,
                ) {
                    // shouldn't happen unless neighbor in the receiving source route
                    // was deleted before receiving the response
                    // making this node isolated because the neighbor
                    // isn't connected to the root anymore.

                    let present = vicinity_graph
                        .nodes()
                        .any(|vicinity_node| vicinity_node == source);
                    tracing::warn!(
                        target: "precompute_paths_and_path_ids",
                        %source,
                        err_msg=%err,
                        neighbors=?underlay_contacts,
                        %present,
                        "Updating underlay information failed"
                    );
                    return Ok(());
                }
                self.vicinity_changed = true;

                // don' track neighbors of 3 hops
                // use root_distance over source-route hops because maybe
                // we discovered a better path to source while the request was pending
                if vicinity_graph
                    .root_distance(&source)
                    .expect("should be present in the vicinity graph")
                    >= VICINITY_RADIUS
                {
                    if self.config.update_interval.is_none() {
                        self.precompute_paths_and_ids(context);
                    }
                    return Ok(());
                }

                // update observed_ssn for all underlay_contacts to initiate a (potential) (re)sync.
                for contact in underlay_contacts.iter() {
                    let nid = contact.id();
                    let ssn = contact.state_seq_nr();

                    // update observed_ssn if newer observed in rtable_data
                    if let Some(observed_ssn) = vicinity_graph.observed_ssn(nid).copied() {
                        if ssn > &observed_ssn {
                            vicinity_graph.update_observed_ssn(nid, *ssn);
                            tracing::trace!(
                                target: "precompute_paths_and_path_ids",
                                %source,
                                node=%nid,
                                old_observed_ssn=%observed_ssn,
                                new_observed_ssn=%ssn,
                                "higher observed state sequence number of neighbor updated"
                            );
                        }
                    } else {
                        // untracked vicinity node: track observed_ssn => insert
                        assert!(
                            vicinity_graph.insert(*nid, &source, *ssn).is_ok(),
                            "RTableData contacts should be connected to source inside vicinity"
                        );
                        tracing::trace!(
                            target: "precompute_paths_and_path_ids",
                            %source,
                            node=%nid,
                            observed_ssn=%ssn,
                            "inserted new node into vicinity graph"
                        );

                        debug_assert!(
                            vicinity_graph
                                .root_distance(nid)
                                .is_some_and(|d| d <= VICINITY_RADIUS),
                            "contacts of source inside vicinity should be part of the vicinity graph"
                        )
                    }
                }
                mem::drop(vicinity_graph);

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
