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
            vicinity_radius: 2,
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
    fn gen_entries_from_graph(&self, context: &C, graph: &VicinityGraph) -> HashSet<PathIdEntry> {
        if !graph.neighbors.is_empty() {
            log::trace!(target: "precompute_paths_and_path_ids", "{:?}", graph);
        }

        let mut entries = HashSet::new();
        for in_path in graph {
            debug_assert!(
                in_path.size() <= self.config.vicinity_radius + 1,
                "VicinityGraph should only generate Paths inside the Vicinity-Radius"
            );

            let Ok(out_path) = in_path.clone().into_iter().skip(1).collect() else {
                log::warn!(target: "precompute_paths_and_path_ids", "VicinityGraph generated path of size 1");
                continue;
            };

            let Some(ulnid) = context.un_table().get(out_path.first()).cloned() else {
                log::warn!(target: "precompute_paths_and_path_ids", "VicinityGraph generated path over invalid neighbor {:?}", out_path.first());
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

    fn precompute_paths_and_ids(&mut self, context: &C) {
        if !self.vicinity_changed {
            return;
        }

        let mut old_entries = self.gen_entries_from_graph(context, &self.old_graph);
        let mut new_entries = self.gen_entries_from_graph(context, &self.vicinity_graph);
        log::trace!(target: "precompute_paths_and_path_ids", "Calculated paths: {:?}", new_entries);
        // Remove intersection
        for new_entry in new_entries.clone() {
            if old_entries.remove(&new_entry) {
                new_entries.remove(&new_entry);
            }
        }
        // Now in old_entries only the removed paths are present
        // and in new_entries only the newly added paths are present
        log::debug!(target: "precompute_paths_and_path_ids", "New paths: {:?}", new_entries);
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
        if let UseCaseEvent::Message(ProtocolMessage::PNDiscReq(ref rtable_data), _) = event {
            tracing::trace!(
                target: "precompute_paths_and_path_ids",
                source_route = ?rtable_data.source_route,
                "Received PNDiscReq"
            );
        }
        match event {
            UseCaseEvent::Message(ProtocolMessage::PNDiscReq(rtable_data), _)
            | UseCaseEvent::Message(ProtocolMessage::PNDiscRsp(rtable_data), _)
            | UseCaseEvent::Message(ProtocolMessage::QueryRouteRsp(rtable_data), _) => {
                // Skip everything not in configured vicinity radius
                if rtable_data.source_route.size() > self.config.vicinity_radius {
                    return Ok(());
                }

                let source = *rtable_data.source();
                let source_ssn = rtable_data.source_state_seq_nr;

                log::trace!(target: "precompute_paths_and_path_ids", "{} has rtable data: {:?}", source, rtable_data.data);

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
                                log::trace!(target: "precompute_paths_and_path_ids", "Ignoring underlay neighbor update of {} as we expected newer data: {:?}", source, rtable_data.data);
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
                                log::trace!(target: "precompute_paths_and_path_ids", "Ignoring underlay neighbor update of {} without any new data: {:?}", source, rtable_data.data);
                                return Ok(());
                            }
                        }
                    }
                } else {
                    // look at the routing table to determine if more recent data
                    if let Some(rt_contact) = context.routing_table().contact(&source) {
                        // `<` ok, since we add the node only once for the first time
                        if &source_ssn < rt_contact.state_seq_nr() {
                            log::trace!(target: "precompute_paths_and_path_ids", "Not adding new node {} to vicinity graph, because its underlay neighbor data is outdated: {:?}", source, rtable_data.data);
                            // not adding node itself to allow checking this bootstrapping condition again
                            return Ok(());
                        }
                    } else {
                        // TODO if we don't have a contact this should probably error out
                        log::error!(target: "precompute_paths_and_path_ids", "No contact for node {}, not adding to vicinity graph", source);
                        log::debug!(target: "precompute_paths_and_path_ids", "Routing table: {:?}", context.routing_table().iter().collect::<Vec<_>>());
                        log::debug!(target: "precompute_paths_and_path_ids", "Underlay neighbor table: {:?}", context.un_table().iter().collect::<Vec<_>>());
                        log::debug!(target: "precompute_paths_and_path_ids", "Vicinity graph: {:?}", self.vicinity_graph);
                    }
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
                    log::debug!(target: "precompute_paths_and_path_ids", "Updated underlay neighbors of {} to {:?}", source, underlay_neighbors_of_source);

                    self.vicinity_changed = true;
                }
            }
            UseCaseEvent::Contact(ContactEvent::Removed(contact)) => {
                // Skip everything not in configured vicinity radius
                if contact.path().size() > self.config.vicinity_radius {
                    return Ok(());
                }
                self.vicinity_changed |= self.vicinity_graph.remove(contact.id()).is_some();
            }
            UseCaseEvent::Contact(ContactEvent::New(contact)) => {
                // only add underlay neighbors
                if !contact.is_pn() {
                    tracing::trace!(target: "precompute_paths_and_path_ids", "Not adding non-underlay neighbor contact: {:?}", contact);
                    return Ok(());
                }

                // other neighbors in the vicinity are added
                // as they are discovered in `RTableData` of ProtocolMessages

                self.vicinity_graph
                    .add(*context.root_id(), HashSet::from([*contact.id()]));
                self.vicinity_changed = true;
            }
            UseCaseEvent::Contact(ContactEvent::Updated { old, new }) => {
                if old.path().size() <= self.config.vicinity_radius
                    && new.path().size() > self.config.vicinity_radius
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
                            log::trace!(target: "precompute_paths_and_path_ids", "Update expected state sequence number of node {}: {}", node_id, expected_ssn);
                            *previous_expected_ssn = expected_ssn;
                        }
                    } else {
                        log::trace!(target: "precompute_paths_and_path_ids", "Add node to resynchronisation queue: {}", node_id);
                        resync_queue.insert(node_id, expected_ssn);
                    }
                } else {
                    log::warn!(target: "precompute_paths_and_path_ids", "Not responding to ResyncNode event in this state: {:?}", self.state);
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
