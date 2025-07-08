use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::ops::Deref;
use std::time::Duration;
use tracing::{instrument, Level};

use crate::domain::protocol_event::forwarding::{
    PathIdEntry, PathIdForwardingEntry, PathIdTableUpdate,
};
use crate::domain::{ContactState, Hasher, NodeId, RoutingTable, StateSeqNr, UnderlayNeighborId};
use crate::messaging::ProtocolMessage;
use crate::use_cases::{
    ContactEvent, EventHandler, NeverError, TimerId, UseCase, UseCaseContext, UseCaseEvent,
    UseCaseRuntime, UseCaseState,
};
use crate::utils::vicinity_graph::VicinityGraph;

use super::ApiEvent;

/// Configuration for [UseCase] [PrecomputePathIds].
#[derive(Debug)]
pub struct PrecomputePathIdsConfig {
    /// Radius of the underlay neighborhood to precompute paths and
    /// [PathIds](crate::domain::PathId) for.
    ///
    /// Its assumed, that all contacts in this radius are also included in the
    /// [RoutingTable].
    pub vicinity_radius: usize,
    /// Interval in which the precomputation will take place.
    ///
    /// If [None] is passed the precomputation will happen on every change.
    pub update_interval: Option<Duration>,
    /// Hasher to use for generation of [PathIds](crate::domain::PathId) from [Path]s.
    pub hasher: Hasher,
}

impl Default for PrecomputePathIdsConfig {
    fn default() -> Self {
        Self {
            vicinity_radius: 3,
            update_interval: None,
            hasher: Hasher::default(),
        }
    }
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
    vicinity_graph: VicinityGraph,
    old_graph: VicinityGraph,
    vicinity_changed: bool,
}

impl<C, const BUCKET_SIZE: usize> PrecomputePathIds<C, BUCKET_SIZE> {
    pub fn new(root_id: NodeId, config: PrecomputePathIdsConfig) -> Self {
        Self {
            _pd: PhantomData,
            state: PrecomputeState::default(),
            config,
            vicinity_graph: VicinityGraph::new(root_id),
            old_graph: VicinityGraph::new(root_id),
            vicinity_changed: false,
        }
    }
}

impl<C, const BUCKET_SIZE: usize> PrecomputePathIds<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    C::UnderlayNeighborTable: Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
{
    #[instrument(
        level = Level::TRACE,
        target = "precompute_paths_and_path_ids",
        skip(self, context),
        fields(?graph),
        ret
    )]
    fn gen_entries_from_graph(&self, context: &C, graph: &VicinityGraph) -> HashSet<PathIdEntry> {
        let mut entries = HashSet::new();
        for in_path in graph {
            debug_assert!(
                in_path.size() <= self.config.vicinity_radius,
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

        let mut old_entries = self.gen_entries_from_graph(context, &self.old_graph);
        let mut new_entries = self.gen_entries_from_graph(context, &self.vicinity_graph);
        // Remove intersection
        for new_entry in new_entries.clone() {
            if old_entries.remove(&new_entry) {
                new_entries.remove(&new_entry);
            }
        }
        // Now in old_entries only the removed paths are present
        // and in new_entries only the newly added paths are present
        tracing::debug!(target: "precompute_paths_and_path_ids", ?new_entries, ?old_entries, "changes of paths in the vicinity");
        for old_entry in old_entries {
            let old_in_path_id = match old_entry {
                PathIdEntry::Forward(entry) => entry.in_path_id,
                PathIdEntry::Decapsulate(entry) => entry.in_path_id,
            };

            context
                .runtime()
                .update_fwd_tables(PathIdTableUpdate::Remove(old_in_path_id));
        }
        for new_entry in new_entries {
            context
                .runtime()
                .update_fwd_tables(PathIdTableUpdate::CreateOrUpdate(new_entry));
        }

        self.old_graph = self.vicinity_graph.clone();
        self.vicinity_changed = false;
    }
}

impl<C, const BUCKET_SIZE: usize> EventHandler for PrecomputePathIds<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: UseCaseRuntime,
    C::UnderlayNeighborTable: Deref<Target = HashMap<NodeId, UnderlayNeighborId>>,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
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
                // Skip everything not in configured vicinity radius
                if rtable_data.source_route.size() >= self.config.vicinity_radius {
                    return Ok(());
                }

                let source = *rtable_data.source();
                let source_ssn = rtable_data.source_state_seq_nr;

                tracing::trace!(target: "precompute_paths_and_path_ids", %source, neighbors=?rtable_data.data, "underlay information updates");

                if self.vicinity_graph.contains(&source) {
                    // check if we received more recent data by sent ssn
                    // first check by expected ssn
                    let sync_requested = if let Some(resync_queue) = self.state.resync_queue_mut() {
                        if let std::collections::hash_map::Entry::Occupied(entry) =
                            resync_queue.entry(source)
                        {
                            let expected_ssn = entry.get();
                            // nothing new about the neighbor
                            if &source_ssn < expected_ssn {
                                tracing::trace!(target: "precompute_paths_and_path_ids", %source, "Ignoring underlay neighbor update as we expected newer data");
                                return Ok(());
                            }

                            // delete entry since we resynchronised successfully
                            entry.remove();

                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    };

                    // if no expected ssn, check the routing table
                    // this usually results in denial
                    if !sync_requested {
                        // look at the routing table to determine if more recent data
                        if let Some(rt_contact) = context.routing_table().contact(&source) {
                            // nothing new about the neighbor
                            if &source_ssn <= rt_contact.state_seq_nr() {
                                tracing::trace!(target: "precompute_paths_and_path_ids", %source, "Ignoring underlay neighbor update without any new data");
                                return Ok(());
                            }
                        }
                    }
                } else {
                    // look at the routing table to determine if more recent data
                    if let Some(rt_contact) = context.routing_table().contact(&source) {
                        // `<` ok, since we add the node only once for the first time
                        if &source_ssn < rt_contact.state_seq_nr() {
                            tracing::trace!(
                                target: "precompute_paths_and_path_ids",
                                %source,
                                %source_ssn,
                                rt_ssn=%rt_contact.state_seq_nr(),
                                "Not adding new node to vicinity graph, because its underlay neighbor data is outdated"
                            );

                            // not adding node itself to allow checking this bootstrapping condition again
                            return Ok(());
                        }
                    }

                    // NOTE: It's not a failure if the contact isn't in the routing table.
                    //   That's why we have the vicinity graph in the first place.
                }

                // update underlay neighbors of source
                let underlay_neighbors_of_source = rtable_data
                    .data
                    .contacts
                    .into_iter()
                    .map(|contact| *contact.id())
                    .collect::<HashSet<_>>();

                let previous = self
                    .vicinity_graph
                    .insert(source, underlay_neighbors_of_source.clone());
                if previous.is_none() || previous.as_ref() != Some(&underlay_neighbors_of_source) {
                    tracing::debug!(
                        target: "precompute_paths_and_path_ids",
                        %source,
                        neighbors=?underlay_neighbors_of_source,
                        "Updated underlay neighbors"
                    );

                    self.vicinity_changed = true;
                }
            }
            UseCaseEvent::Contact(ContactEvent::Removed(contact)) => {
                // Skip everything not in configured vicinity radius
                if contact.path().size() >= self.config.vicinity_radius {
                    return Ok(());
                }
                self.vicinity_changed |= self.vicinity_graph.remove(contact.id()).is_some();
            }
            UseCaseEvent::Contact(ContactEvent::New(contact)) => {
                // only add underlay neighbors
                if !contact.is_uln() {
                    tracing::trace!(target: "precompute_paths_and_path_ids", %contact, "Not adding non-underlay neighbor contact");
                    return Ok(());
                }

                // other neighbors in the vicinity are added
                // as they are discovered in `RTableData` of ProtocolMessages

                self.vicinity_graph
                    .add(*context.root_id(), HashSet::from([*contact.id()]));
                self.vicinity_changed = true;
            }
            UseCaseEvent::Contact(ContactEvent::Updated { old, new }) => {
                if old.path().size() < self.config.vicinity_radius
                    && new.path().size() >= self.config.vicinity_radius
                {
                    // Contact was changed to out of vicinity
                    self.vicinity_graph.remove(new.id());
                    self.vicinity_changed = true;
                }
                // Contact changed to inside vicinity will be handled by vicinity discovery
                else if old.state() == &ContactState::Valid && new.state() != &ContactState::Valid
                {
                    let changed = self.vicinity_graph.invalidate(new.id());
                    self.vicinity_changed = changed;
                } else if old.state() != &ContactState::Valid && new.state() == &ContactState::Valid
                {
                    let changed = self.vicinity_graph.validate(new.id());
                    self.vicinity_changed = changed;
                }
            }
            UseCaseEvent::Timer(timer_id) => {
                if let PrecomputeState::Waiting {
                    timer_id: own_id, ..
                } = self.state
                {
                    if timer_id == own_id {
                        self.precompute_paths_and_ids(context);
                    }
                    return Ok(());
                }
            }
            UseCaseEvent::API(ApiEvent::VicinityGraph(sender)) => {
                let _ = sender.send(format!("{:#?}", self.vicinity_graph));
            }
            UseCaseEvent::ResyncNode(node_id, expected_ssn) => {
                // TODO deduplicate state "shared" with Vicinity Discovery
                if let Some(resync_queue) = self.state.resync_queue_mut() {
                    // only update expected_ssn, if greater
                    if let Some(previous_expected_ssn) = resync_queue.get_mut(&node_id) {
                        if *previous_expected_ssn < expected_ssn {
                            tracing::trace!(
                                target: "precompute_paths_and_path_ids",
                                node=%node_id,
                                %expected_ssn,
                                %previous_expected_ssn,
                                "Update expected state sequence number of node"
                            );
                            *previous_expected_ssn = expected_ssn;
                        }
                    } else {
                        tracing::trace!(target: "precompute_paths_and_path_ids", node=%node_id, "Add node to resynchronisation queue");
                        resync_queue.insert(node_id, expected_ssn);
                    }

                    tracing::trace!(target: "precompute_paths_and_path_ids", ?resync_queue, "Changes in Resync Queue");
                } else {
                    tracing::warn!(target: "precompute_paths_and_path_ids", "Not responding to ResyncNode event in this state");
                }
            }
            _ => {}
        }

        if self.config.update_interval.is_none() {
            self.precompute_paths_and_ids(context);
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
{
    type State = PrecomputeState;

    fn start(&mut self, context: &C) -> Result<(), Self::Error> {
        if let Some(duration) = self.config.update_interval {
            let timer_id = context.runtime().register_periodic_timer(duration);
            self.state = PrecomputeState::Waiting {
                timer_id,
                resync_queue: HashMap::default(),
            };
        }

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum PrecomputeState {
    Idle {
        resync_queue: HashMap<NodeId, StateSeqNr>,
    },
    Waiting {
        timer_id: TimerId,
        resync_queue: HashMap<NodeId, StateSeqNr>,
    },
    Error,
}
#[allow(dead_code)]
impl PrecomputeState {
    fn resync_queue(&self) -> Option<&HashMap<NodeId, StateSeqNr>> {
        match self {
            Self::Idle { resync_queue, .. } => Some(resync_queue),
            Self::Waiting { resync_queue, .. } => Some(resync_queue),
            _ => None,
        }
    }

    fn resync_queue_mut(&mut self) -> Option<&mut HashMap<NodeId, StateSeqNr>> {
        match self {
            Self::Idle { resync_queue, .. } => Some(resync_queue),
            Self::Waiting { resync_queue, .. } => Some(resync_queue),
            _ => None,
        }
    }
}

impl Default for PrecomputeState {
    fn default() -> Self {
        Self::Idle {
            resync_queue: HashMap::default(),
        }
    }
}

impl UseCaseState for PrecomputeState {
    fn is_error(&self) -> bool {
        self == &Self::Error
    }
}
