use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::marker::PhantomData;
use std::ops::Deref;
use std::time::Duration;

use crate::context::UseCaseContext;
use crate::domain::{
    ContactState, EmptyPathError, NetworkInterface, NodeId, Path, RoutingTable, StateSeqNr,
};
use crate::forwarding::hasher::Hasher;
use crate::forwarding::{ForwardingTables, PathIdEntry, PathIdForwardingEntry, PathIdTable};
use crate::messaging::ProtocolMessage;
use crate::runtime::UseCaseRuntime;
use crate::use_cases::{ContactEvent, EventHandler, TimerId, UseCase, UseCaseEvent, UseCaseState};
use crate::utils::vicinity_graph::VicinityGraph;

use super::ApiEvent;

/// Configuration for [UseCase] [PrecomputePathIds].
#[derive(Debug)]
pub struct PrecomputePathIdsConfig {
    /// Radius of the physical neighborhood to precompute paths and
    /// [PathIds](crate::domain::PathId) for.
    ///
    /// Its assumed, that all contacts in this radius are also included in the
    /// [RoutingTable](crate::domain::RoutingTable).
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
            _pd: PhantomData::default(),
            state: PrecomputeState::default(),
            config,
            vicinity_graph: VicinityGraph::new(root_id.clone()),
            old_graph: VicinityGraph::new(root_id),
            vicinity_changed: false,
        }
    }
}

impl<C, const BUCKET_SIZE: usize> PrecomputePathIds<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::ForwardingTables: PathIdTable,
    <<C as UseCaseContext>::ForwardingTables as PathIdTable>::Error: Display,
    C::PhysicalNeighborTable: Deref<Target = HashMap<NodeId, NetworkInterface>>,
{
    fn gen_entries_from_graph(&self, context: &C, graph: &VicinityGraph) -> HashSet<PathIdEntry> {
        log::debug!(target: "precompute_paths_and_path_ids", "{:?}", graph);
        let mut entries = HashSet::new();
        for in_path in graph {
            let out_path =
                Result::<Path, EmptyPathError>::from_iter(in_path.clone().into_iter().skip(1));
            if out_path.is_err() {
                log::warn!(target: "precompute_paths_and_path_ids", "VicinityGraph generated path of size 1");
                continue;
            }
            let out_path = out_path.unwrap();

            // FIXME don't install longer paths than vicinity radius
            if out_path.size() - 1 > self.config.vicinity_radius {
                continue;
            }

            let interface = context.pn_table().get(out_path.first()).cloned();
            if interface.is_none() {
                log::warn!(target: "precompute_paths_and_path_ids", "VicinityGraph generated path over invalid neighbor");
                continue;
            }
            let entry = PathIdEntry::Forward(PathIdForwardingEntry {
                in_path_id: self.config.hasher.hash(&in_path),
                out_path_id: self.config.hasher.hash(&out_path),
                next_hop: out_path.first().clone(),
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
        // Remove intersection
        for new_entry in new_entries.clone() {
            if old_entries.remove(&new_entry) {
                new_entries.remove(&new_entry);
            }
        }
        // Now in old_entries only the removed paths are present
        // and in new_entries only the newly added paths are present
        let mut fwd_tables = context.forwarding_tables_mut();
        for old_entry in old_entries {
            let old_in_path_id = match old_entry {
                PathIdEntry::Forward(entry) => entry.in_path_id,
                PathIdEntry::Decapsulate(entry) => entry.in_path_id,
            };
            if let Err(e) = fwd_tables.remove(&old_in_path_id) {
                log::error!(target: "precompute_paths_and_path_ids", "Failed to remove entry for PathID {}: {}", old_in_path_id, e);
            }
        }
        for new_entry in new_entries {
            let insertion_result = fwd_tables
                .create_or_update(new_entry.clone())
                .or_else(|_| fwd_tables.create(new_entry.clone()));
            if let Err(e) = insertion_result {
                log::error!(target: "precompute_paths_and_path_ids", "Failed to create entry {}: {}", new_entry, e);
            }
        }

        self.old_graph = self.vicinity_graph.clone();
        self.vicinity_changed = false;
    }
}

impl<C, const BUCKET_SIZE: usize> EventHandler for PrecomputePathIds<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::ForwardingTables: PathIdTable,
    <<C as UseCaseContext>::ForwardingTables as PathIdTable>::Error: Display,
    C::PhysicalNeighborTable: Deref<Target = HashMap<NodeId, NetworkInterface>>,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
{
    type Context = C;
    type Error = ();
    type Value = ();

    fn handle_event(
        &mut self,
        context: &Self::Context,
        event: UseCaseEvent,
    ) -> Result<Self::Value, Self::Error> {
        match event {
            UseCaseEvent::Message(ProtocolMessage::PNDiscReq(rtable_data), _)
            | UseCaseEvent::Message(ProtocolMessage::PNDiscRsp(rtable_data), _)
            | UseCaseEvent::Message(ProtocolMessage::QueryRouteRsp(rtable_data), _) => {
                // Skip everything not in configured vicinity radius
                if rtable_data.source_route.size() - 1 > self.config.vicinity_radius {
                    return Ok(());
                }

                let source = rtable_data.source().clone();
                let source_ssn = rtable_data.source_state_seq_nr;

                if self.vicinity_graph.contains(&source) {
                    // check if we received more recent data by sent ssn
                    // first check by expected ssn
                    let sync_requested = if let Some(resync_queue) = self.state.resync_queue_mut() {
                        if let std::collections::hash_map::Entry::Occupied(entry) =
                            resync_queue.entry(source.clone())
                        {
                            let expected_ssn = entry.get();
                            // nothing new about the neighbor
                            if &source_ssn < expected_ssn {
                                log::trace!(target: "precompute_paths_and_path_ids", "Ignoring physical neighbor update of {} as we expected newer data: {:?}", source, rtable_data);
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
                                log::trace!(target: "precompute_paths_and_path_ids", "Ignoring physical neighbor update of {} without any new data: {:?}", source, rtable_data);
                                return Ok(());
                            }
                        }
                    }
                } else {
                    // look at the routing table to determine if more recent data
                    if let Some(rt_contact) = context.routing_table().contact(&source) {
                        // `<` ok, since we add the node only once for the first time
                        if &source_ssn < rt_contact.state_seq_nr() {
                            log::trace!(target: "precompute_paths_and_path_ids", "Not adding new node {} to vicinity graph, because its physical neighbor data is outdated: {:?}", source, rtable_data);
                            // not adding node itself to allow checking this bootstrapping condition again
                            return Ok(());
                        }
                    }
                    // TODO if we don't have a contact this should probably error out
                    log::trace!(target: "precompute_paths_and_path_ids", "Adding new node to vicinity graph: {} ", source);
                }

                // update physical neighbors of source
                let physical_neighbors_of_source = rtable_data
                    .data
                    .contacts
                    .into_iter()
                    .map(|contact| contact.id().clone())
                    .collect::<HashSet<_>>();

                let previous = self
                    .vicinity_graph
                    .insert(source.clone(), physical_neighbors_of_source.clone());
                if previous.is_none() || previous.as_ref() != Some(&physical_neighbors_of_source) {
                    log::trace!(target: "precompute_paths_and_path_ids", "Updated physical neighbors of {} to {:?}", source, physical_neighbors_of_source);

                    self.vicinity_changed = true;

                    if self.config.update_interval.is_none() {
                        self.precompute_paths_and_ids(context);
                    }
                }
            }
            UseCaseEvent::Contact(ContactEvent::Removed(contact)) => {
                // Skip everything not in configured vicinity radius
                if contact.path().size() > self.config.vicinity_radius {
                    return Ok(());
                }
                self.vicinity_changed |= self.vicinity_graph.remove(contact.id()).is_some();
                if self.config.update_interval.is_none() {
                    self.precompute_paths_and_ids(context);
                }
            }
            UseCaseEvent::Contact(ContactEvent::New(contact)) => {
                // only add physical neighbors
                if !contact.is_pn() {
                    return Ok(());
                }

                // other neighbors in the vicinity are added
                // as they are discovered in `RTableData` of ProtocolMessages

                self.vicinity_graph.add(
                    context.root_id().clone(),
                    HashSet::from([contact.id().clone()]),
                );
                self.vicinity_changed = true;

                if self.config.update_interval.is_none() {
                    self.precompute_paths_and_ids(context);
                }
            }
            UseCaseEvent::Contact(ContactEvent::Updated { old, new }) => {
                if old.path().size() <= self.config.vicinity_radius
                    && new.path().size() > self.config.vicinity_radius
                {
                    // Contact was changed to out of vicinity
                    self.vicinity_graph.remove(new.id());
                    self.vicinity_changed = true;
                    return Ok(());
                }
                // Contact changed to inside vicinity will be handled by vicinity discovery

                if old.state() == &ContactState::Valid && new.state() != &ContactState::Valid {
                    let changed = self.vicinity_graph.invalidate(new.id());
                    self.vicinity_changed = changed;
                    return Ok(());
                }
                if old.state() != &ContactState::Valid && new.state() == &ContactState::Valid {
                    let changed = self.vicinity_graph.validate(new.id());
                    self.vicinity_changed = changed;
                    return Ok(());
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
                }
            }
            UseCaseEvent::API(ApiEvent::VicinityGraph(sender)) => {
                sender
                    .send(format!("{:#?}", self.vicinity_graph))
                    .map_err(|_| ())?;
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

        Ok(())
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase for PrecomputePathIds<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::ForwardingTables: ForwardingTables,
    C::Runtime: UseCaseRuntime,
    <<C as UseCaseContext>::ForwardingTables as PathIdTable>::Error: Display,
    C::PhysicalNeighborTable: Deref<Target = HashMap<NodeId, NetworkInterface>>,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE>,
{
    type State = PrecomputeState;

    fn start(&mut self, context: &Self::Context) -> Result<(), Self::Error> {
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::context::{ContextConfig, SyncContext, UseCaseContext};
    use crate::domain::single_bucket::SingleBucketRT;
    use crate::domain::{
        Contact, ContactState, EmptyPathError, InMemoryPNTable, InsertionStrategyResult,
        NetworkInterface, NodeId, PNTable, Path, RoutingTable, StateSeqNr, TestInsertionStrategy,
    };
    use crate::forwarding::hasher::Hasher;
    use crate::forwarding::in_memory_tables::InMemoryFwdTables;
    use crate::forwarding::PathIdEntry;
    use crate::messaging::source_route::SourceRoute;
    use crate::messaging::{
        InMemoryMessageChannel, Nonce, ProtocolMessage, RTableData, ReqRspMessage,
    };
    use crate::runtime::ImmediateRuntime;
    use crate::use_cases::precompute_paths_and_path_ids::{
        PrecomputePathIds, PrecomputePathIdsConfig,
    };
    use crate::use_cases::{ContactEvent, EventHandler, UseCase, UseCaseEvent};

    #[test]
    fn precompute_all_paths_in_vicinity() {
        crate::tests::init();

        /*
        Topology:
            root_id: 1

             /- 2 -\
           1 -- 3 -- 4 -- 5
                |         |
             `- 6 -- 7 -´

        */

        let interface_two = NetworkInterface::with_name("1--2");
        let interface_three = NetworkInterface::with_name("1--3");
        let interface_six = NetworkInterface::with_name("1--6");

        let root_id = NodeId::with_msb(1);

        let contacts = [
            Contact::new(Path::from(NodeId::with_msb(1)), StateSeqNr::from(1)),
            Contact::new(Path::from(NodeId::with_msb(2)), StateSeqNr::from(2)),
            Contact::new(Path::from(NodeId::with_msb(3)), StateSeqNr::from(3)),
            Contact::new(Path::from(NodeId::with_msb(4)), StateSeqNr::from(4)),
            Contact::new(Path::from(NodeId::with_msb(5)), StateSeqNr::from(5)),
            Contact::new(Path::from(NodeId::with_msb(6)), StateSeqNr::from(6)),
            Contact::new(Path::from(NodeId::with_msb(7)), StateSeqNr::from(7)),
        ];

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(30);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let mut routing_table = SingleBucketRT::<20>::new(root_id.clone());
        assert!(routing_table.insert(contacts[1].clone()).is_ok());
        assert!(routing_table.insert(contacts[2].clone()).is_ok());
        assert!(routing_table.insert(contacts[5].clone()).is_ok());

        let mut pn_table = InMemoryPNTable::new();
        pn_table.insert(contacts[1].id().clone(), interface_two.clone());
        pn_table.insert(contacts[2].id().clone(), interface_three.clone());
        pn_table.insert(contacts[5].id().clone(), interface_six.clone());

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

        let hasher = Hasher::default();
        let config = PrecomputePathIdsConfig {
            vicinity_radius: 3,
            update_interval: None,
            hasher: hasher.clone(),
        };
        let mut use_case = PrecomputePathIds::new(root_id, config);
        assert!(
            use_case.start(&sync_context).is_ok(),
            "failed to start use case"
        );

        let events = [
            // Contact events to add physical neighbors
            UseCaseEvent::Contact(ContactEvent::New(contacts[1].clone())),
            UseCaseEvent::Contact(ContactEvent::New(contacts[2].clone())),
            UseCaseEvent::Contact(ContactEvent::New(contacts[5].clone())),
            // Message from Node 2 to 1 with neighbors 1 and 4
            UseCaseEvent::Message(
                ProtocolMessage::PNDiscReq(ReqRspMessage {
                    nonce: Nonce::random(),
                    source_state_seq_nr: StateSeqNr::from(2),
                    data: RTableData {
                        contacts: vec![contacts[0].clone(), contacts[3].clone()],
                    },
                    not_via: Default::default(),
                    source_route: SourceRoute::from(Path::from([
                        NodeId::with_msb(2),
                        NodeId::with_msb(1),
                    ])),
                }),
                interface_two.clone(),
            ),
            // Message from Node 3 to 1 with neighbors 1, 4 and 6
            UseCaseEvent::Message(
                ProtocolMessage::PNDiscReq(ReqRspMessage {
                    nonce: Nonce::random(),
                    source_state_seq_nr: StateSeqNr::from(3),
                    data: RTableData {
                        contacts: vec![
                            contacts[0].clone(),
                            contacts[3].clone(),
                            contacts[5].clone(),
                        ],
                    },
                    not_via: Default::default(),
                    source_route: SourceRoute::from(Path::from([
                        NodeId::with_msb(3),
                        NodeId::with_msb(1),
                    ])),
                }),
                interface_three.clone(),
            ),
            // Message from Node 6 to 1 with neighbors 1, 3 and 7
            UseCaseEvent::Message(
                ProtocolMessage::PNDiscReq(ReqRspMessage {
                    nonce: Nonce::random(),
                    source_state_seq_nr: StateSeqNr::from(6),
                    data: RTableData {
                        contacts: vec![
                            contacts[0].clone(),
                            contacts[2].clone(),
                            contacts[6].clone(),
                        ],
                    },
                    not_via: Default::default(),
                    source_route: SourceRoute::from(Path::from([
                        NodeId::with_msb(6),
                        NodeId::with_msb(1),
                    ])),
                }),
                interface_six.clone(),
            ),
            // Message from Node 4 to 1 with neighbors 2, 3 and 5
            UseCaseEvent::Message(
                ProtocolMessage::PNDiscReq(ReqRspMessage {
                    nonce: Nonce::random(),
                    source_state_seq_nr: StateSeqNr::from(4),
                    data: RTableData {
                        contacts: vec![
                            contacts[1].clone(),
                            contacts[2].clone(),
                            contacts[4].clone(),
                        ],
                    },
                    not_via: Default::default(),
                    source_route: SourceRoute::from(Path::from([
                        NodeId::with_msb(4),
                        NodeId::with_msb(2),
                        NodeId::with_msb(1),
                    ]))
                    .advanced(),
                }),
                interface_two.clone(),
            ),
            // Message from Node 7 to 1 with neighbors 6 and 5
            UseCaseEvent::Message(
                ProtocolMessage::PNDiscReq(ReqRspMessage {
                    nonce: Nonce::random(),
                    source_state_seq_nr: StateSeqNr::from(7),
                    data: RTableData {
                        contacts: vec![contacts[5].clone(), contacts[4].clone()],
                    },
                    not_via: Default::default(),
                    source_route: SourceRoute::from(Path::from([
                        NodeId::with_msb(7),
                        NodeId::with_msb(6),
                        NodeId::with_msb(1),
                    ]))
                    .advanced(),
                }),
                interface_six.clone(),
            ),
            // Message from Node 5 to 1 with neighbors 4 and 7
            UseCaseEvent::Message(
                ProtocolMessage::PNDiscReq(ReqRspMessage {
                    nonce: Nonce::random(),
                    source_state_seq_nr: StateSeqNr::from(5),
                    data: RTableData {
                        contacts: vec![contacts[3].clone(), contacts[6].clone()],
                    },
                    not_via: Default::default(),
                    source_route: SourceRoute::from(Path::from([
                        NodeId::with_msb(5),
                        NodeId::with_msb(4),
                        NodeId::with_msb(3),
                        NodeId::with_msb(1),
                    ]))
                    .advanced()
                    .advanced(),
                }),
                interface_three.clone(),
            ),
        ];

        for event in events {
            let result = use_case.handle_event(&sync_context, event);
            assert!(result.is_ok(), "Failed to handle ");
        }

        let expected_paths_with_interface = Vec::from([
            // All paths to 2
            (
                Path::from([NodeId::with_msb(1), NodeId::with_msb(2)]),
                interface_two.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(3),
                    NodeId::with_msb(4),
                    NodeId::with_msb(2),
                ]),
                interface_three.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(6),
                    NodeId::with_msb(3),
                    NodeId::with_msb(4),
                    NodeId::with_msb(2),
                ]),
                interface_six.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(6),
                    NodeId::with_msb(7),
                    NodeId::with_msb(5),
                    NodeId::with_msb(4),
                    NodeId::with_msb(2),
                ]),
                interface_six.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(3),
                    NodeId::with_msb(6),
                    NodeId::with_msb(7),
                    NodeId::with_msb(5),
                    NodeId::with_msb(4),
                    NodeId::with_msb(2),
                ]),
                interface_three.clone(),
            ),
            // All paths to 3
            (
                Path::from([NodeId::with_msb(1), NodeId::with_msb(3)]),
                interface_three.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(2),
                    NodeId::with_msb(4),
                    NodeId::with_msb(3),
                ]),
                interface_two.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(2),
                    NodeId::with_msb(4),
                    NodeId::with_msb(5),
                    NodeId::with_msb(7),
                    NodeId::with_msb(6),
                    NodeId::with_msb(3),
                ]),
                interface_two.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(6),
                    NodeId::with_msb(3),
                ]),
                interface_six.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(6),
                    NodeId::with_msb(7),
                    NodeId::with_msb(5),
                    NodeId::with_msb(4),
                    NodeId::with_msb(3),
                ]),
                interface_six.clone(),
            ),
            // All paths to 4
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(2),
                    NodeId::with_msb(4),
                ]),
                interface_two.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(3),
                    NodeId::with_msb(4),
                ]),
                interface_three.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(3),
                    NodeId::with_msb(6),
                    NodeId::with_msb(7),
                    NodeId::with_msb(5),
                    NodeId::with_msb(4),
                ]),
                interface_three.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(6),
                    NodeId::with_msb(3),
                    NodeId::with_msb(4),
                ]),
                interface_six.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(6),
                    NodeId::with_msb(7),
                    NodeId::with_msb(5),
                    NodeId::with_msb(4),
                ]),
                interface_six.clone(),
            ),
            // All paths to 5
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(2),
                    NodeId::with_msb(4),
                    NodeId::with_msb(5),
                ]),
                interface_two.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(2),
                    NodeId::with_msb(4),
                    NodeId::with_msb(3),
                    NodeId::with_msb(6),
                    NodeId::with_msb(7),
                    NodeId::with_msb(5),
                ]),
                interface_two.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(3),
                    NodeId::with_msb(4),
                    NodeId::with_msb(5),
                ]),
                interface_three.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(3),
                    NodeId::with_msb(6),
                    NodeId::with_msb(7),
                    NodeId::with_msb(5),
                ]),
                interface_three.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(6),
                    NodeId::with_msb(7),
                    NodeId::with_msb(5),
                ]),
                interface_six.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(6),
                    NodeId::with_msb(3),
                    NodeId::with_msb(4),
                    NodeId::with_msb(5),
                ]),
                interface_six.clone(),
            ),
            // All paths to 6
            (
                Path::from([NodeId::with_msb(1), NodeId::with_msb(6)]),
                interface_six.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(3),
                    NodeId::with_msb(6),
                ]),
                interface_three.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(3),
                    NodeId::with_msb(4),
                    NodeId::with_msb(5),
                    NodeId::with_msb(7),
                    NodeId::with_msb(6),
                ]),
                interface_three.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(2),
                    NodeId::with_msb(4),
                    NodeId::with_msb(3),
                    NodeId::with_msb(6),
                ]),
                interface_two.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(2),
                    NodeId::with_msb(4),
                    NodeId::with_msb(5),
                    NodeId::with_msb(7),
                    NodeId::with_msb(6),
                ]),
                interface_two.clone(),
            ),
            // All paths to 7
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(6),
                    NodeId::with_msb(7),
                ]),
                interface_six.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(6),
                    NodeId::with_msb(3),
                    NodeId::with_msb(4),
                    NodeId::with_msb(5),
                    NodeId::with_msb(7),
                ]),
                interface_six.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(3),
                    NodeId::with_msb(6),
                    NodeId::with_msb(7),
                ]),
                interface_three.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(3),
                    NodeId::with_msb(4),
                    NodeId::with_msb(5),
                    NodeId::with_msb(7),
                ]),
                interface_three.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(2),
                    NodeId::with_msb(4),
                    NodeId::with_msb(3),
                    NodeId::with_msb(6),
                    NodeId::with_msb(7),
                ]),
                interface_two.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(2),
                    NodeId::with_msb(4),
                    NodeId::with_msb(5),
                    NodeId::with_msb(7),
                ]),
                interface_two.clone(),
            ),
        ]);

        let fwd_table = sync_context.forwarding_tables();
        for (in_path, interface) in expected_paths_with_interface {
            let out_path =
                Result::<Path, EmptyPathError>::from_iter(in_path.clone().into_iter().skip(1))
                    .expect("invalid expected path");
            let in_path_id = hasher.hash(&in_path);
            // FIXME
            let entry = PathIdEntry {
                in_path_id: in_path_id.clone(),
                out_path_id: Some(hasher.hash(&out_path)),
                next_hop: out_path.first().clone(),
            };
            let created_entry = fwd_table.path_id_entry(&in_path_id);
            assert_eq!(
                created_entry,
                Some(&entry),
                "PathIdEntry for {} not created or invalid: {:?}",
                in_path,
                created_entry
            );
        }
    }

    #[test]
    fn remove_paths_on_contact_removal_or_invalidation() {
        crate::tests::init();

        /*
        Topology:
            root_id: 1

             /- 2 -\
           1 -- 3 -- 4 -- 5
                |         |
             `- 6 -- 7 -´

        */

        let interface_two = NetworkInterface::with_name("1--2");
        let interface_three = NetworkInterface::with_name("1--3");
        let interface_six = NetworkInterface::with_name("1--6");

        let root_id = NodeId::with_msb(1);

        let contacts = [
            Contact::new(Path::from(NodeId::with_msb(1)), StateSeqNr::from(1)),
            Contact::new(Path::from(NodeId::with_msb(2)), StateSeqNr::from(2)),
            Contact::new(Path::from(NodeId::with_msb(3)), StateSeqNr::from(3)),
            Contact::new(Path::from(NodeId::with_msb(4)), StateSeqNr::from(4)),
            Contact::new(Path::from(NodeId::with_msb(5)), StateSeqNr::from(5)),
            Contact::new(Path::from(NodeId::with_msb(6)), StateSeqNr::from(6)),
            Contact::new(Path::from(NodeId::with_msb(7)), StateSeqNr::from(7)),
        ];

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(30);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let mut routing_table = SingleBucketRT::<20>::new(root_id.clone());
        assert!(routing_table.insert(contacts[1].clone()).is_ok());
        assert!(routing_table.insert(contacts[2].clone()).is_ok());
        assert!(routing_table.insert(contacts[5].clone()).is_ok());

        let mut pn_table = InMemoryPNTable::new();
        pn_table.insert(contacts[1].id().clone(), interface_two.clone());
        pn_table.insert(contacts[2].id().clone(), interface_three.clone());
        pn_table.insert(contacts[5].id().clone(), interface_six.clone());

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

        let hasher = Hasher::default();
        let config = PrecomputePathIdsConfig {
            vicinity_radius: 3,
            update_interval: None,
            hasher: hasher.clone(),
        };
        let mut use_case = PrecomputePathIds::new(root_id, config);
        assert!(
            use_case.start(&sync_context).is_ok(),
            "failed to start use case"
        );

        let events = [
            // Contact events to add physical neighbors
            UseCaseEvent::Contact(ContactEvent::New(contacts[1].clone())),
            UseCaseEvent::Contact(ContactEvent::New(contacts[2].clone())),
            UseCaseEvent::Contact(ContactEvent::New(contacts[5].clone())),
            // Message from Node 2 to 1 with neighbors 1 and 4
            UseCaseEvent::Message(
                ProtocolMessage::PNDiscReq(ReqRspMessage {
                    nonce: Nonce::random(),
                    source_state_seq_nr: StateSeqNr::from(2),
                    data: RTableData {
                        contacts: vec![contacts[0].clone(), contacts[3].clone()],
                    },
                    not_via: Default::default(),
                    source_route: SourceRoute::from(Path::from([
                        NodeId::with_msb(2),
                        NodeId::with_msb(1),
                    ])),
                }),
                interface_two.clone(),
            ),
            // Message from Node 3 to 1 with neighbors 1, 4 and 6
            UseCaseEvent::Message(
                ProtocolMessage::PNDiscReq(ReqRspMessage {
                    nonce: Nonce::random(),
                    source_state_seq_nr: StateSeqNr::from(3),
                    data: RTableData {
                        contacts: vec![
                            contacts[0].clone(),
                            contacts[3].clone(),
                            contacts[5].clone(),
                        ],
                    },
                    not_via: Default::default(),
                    source_route: SourceRoute::from(Path::from([
                        NodeId::with_msb(3),
                        NodeId::with_msb(1),
                    ])),
                }),
                interface_three.clone(),
            ),
            // Message from Node 6 to 1 with neighbors 1, 3 and 7
            UseCaseEvent::Message(
                ProtocolMessage::PNDiscReq(ReqRspMessage {
                    nonce: Nonce::random(),
                    source_state_seq_nr: StateSeqNr::from(6),
                    data: RTableData {
                        contacts: vec![
                            contacts[0].clone(),
                            contacts[2].clone(),
                            contacts[6].clone(),
                        ],
                    },
                    not_via: Default::default(),
                    source_route: SourceRoute::from(Path::from([
                        NodeId::with_msb(6),
                        NodeId::with_msb(1),
                    ])),
                }),
                interface_six.clone(),
            ),
            // Message from Node 4 to 1 with neighbors 2, 3 and 5
            UseCaseEvent::Message(
                ProtocolMessage::PNDiscReq(ReqRspMessage {
                    nonce: Nonce::random(),
                    source_state_seq_nr: StateSeqNr::from(4),
                    data: RTableData {
                        contacts: vec![
                            contacts[1].clone(),
                            contacts[2].clone(),
                            contacts[4].clone(),
                        ],
                    },
                    not_via: Default::default(),
                    source_route: SourceRoute::from(Path::from([
                        NodeId::with_msb(4),
                        NodeId::with_msb(2),
                        NodeId::with_msb(1),
                    ]))
                    .advanced(),
                }),
                interface_two.clone(),
            ),
            // Message from Node 7 to 1 with neighbors 6 and 5
            UseCaseEvent::Message(
                ProtocolMessage::PNDiscReq(ReqRspMessage {
                    nonce: Nonce::random(),
                    source_state_seq_nr: StateSeqNr::from(7),
                    data: RTableData {
                        contacts: vec![contacts[5].clone(), contacts[4].clone()],
                    },
                    not_via: Default::default(),
                    source_route: SourceRoute::from(Path::from([
                        NodeId::with_msb(7),
                        NodeId::with_msb(6),
                        NodeId::with_msb(1),
                    ]))
                    .advanced(),
                }),
                interface_six.clone(),
            ),
            // Message from Node 5 to 1 with neighbors 4 and 7
            UseCaseEvent::Message(
                ProtocolMessage::PNDiscReq(ReqRspMessage {
                    nonce: Nonce::random(),
                    source_state_seq_nr: StateSeqNr::from(5),
                    data: RTableData {
                        contacts: vec![contacts[3].clone(), contacts[6].clone()],
                    },
                    not_via: Default::default(),
                    source_route: SourceRoute::from(Path::from([
                        NodeId::with_msb(5),
                        NodeId::with_msb(4),
                        NodeId::with_msb(3),
                        NodeId::with_msb(1),
                    ]))
                    .advanced()
                    .advanced(),
                }),
                interface_three.clone(),
            ),
            UseCaseEvent::Contact(ContactEvent::Updated {
                new: {
                    let mut contact_three = contacts[2].clone();
                    *contact_three.state_mut() = ContactState::Invalid;
                    contact_three
                },
                old: contacts[2].clone(),
            }),
            UseCaseEvent::Contact(ContactEvent::Removed(contacts[6].clone())),
        ];

        for event in events {
            let result = use_case.handle_event(&sync_context, event);
            assert!(result.is_ok(), "Failed to handle ");
        }

        // All paths without the ones mentioning node 3 or 7
        let expected_paths_with_interface = Vec::from([
            // All paths to 2
            (
                Path::from([NodeId::with_msb(1), NodeId::with_msb(2)]),
                interface_two.clone(),
            ),
            // All paths to 4
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(2),
                    NodeId::with_msb(4),
                ]),
                interface_two.clone(),
            ),
            // All paths to 5
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(2),
                    NodeId::with_msb(4),
                    NodeId::with_msb(5),
                ]),
                interface_two.clone(),
            ),
            // All paths to 6
            (
                Path::from([NodeId::with_msb(1), NodeId::with_msb(6)]),
                interface_six.clone(),
            ),
        ]);

        let fwd_table = sync_context.forwarding_tables();
        for (in_path, interface) in expected_paths_with_interface {
            let out_path =
                Result::<Path, EmptyPathError>::from_iter(in_path.clone().into_iter().skip(1))
                    .expect("invalid expected path");
            let in_path_id = hasher.hash(&in_path);
            // FIXME
            let entry = PathIdEntry {
                in_path_id: in_path_id.clone(),
                out_path_id: Some(hasher.hash(&out_path)),
                next_hop: out_path.first().clone(),
            };
            let created_entry = fwd_table.path_id_entry(&in_path_id);
            assert_eq!(
                created_entry,
                Some(&entry),
                "PathIdEntry for {} not created or invalid: {:?}",
                in_path,
                created_entry
            );
        }
    }

    #[test]
    fn precompute_for_all_after_invalidation_and_validation() {
        crate::tests::init();

        /*
        Topology:
            root_id: 1

             /- 2 -\
           1 -- 3 -- 4 -- 5
                |         |
             `- 6 -- 7 -´

        */

        let interface_two = NetworkInterface::with_name("1--2");
        let interface_three = NetworkInterface::with_name("1--3");
        let interface_six = NetworkInterface::with_name("1--6");

        let root_id = NodeId::with_msb(1);

        let contacts = [
            Contact::new(Path::from(NodeId::with_msb(1)), StateSeqNr::from(1)),
            Contact::new(Path::from(NodeId::with_msb(2)), StateSeqNr::from(2)),
            Contact::new(Path::from(NodeId::with_msb(3)), StateSeqNr::from(3)),
            Contact::new(Path::from(NodeId::with_msb(4)), StateSeqNr::from(4)),
            Contact::new(Path::from(NodeId::with_msb(5)), StateSeqNr::from(5)),
            Contact::new(Path::from(NodeId::with_msb(6)), StateSeqNr::from(6)),
            Contact::new(Path::from(NodeId::with_msb(7)), StateSeqNr::from(7)),
        ];

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(30);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let mut routing_table = SingleBucketRT::<20>::new(root_id.clone());
        assert!(routing_table.insert(contacts[1].clone()).is_ok());
        assert!(routing_table.insert(contacts[2].clone()).is_ok());
        assert!(routing_table.insert(contacts[5].clone()).is_ok());

        let mut pn_table = InMemoryPNTable::new();
        pn_table.insert(contacts[1].id().clone(), interface_two.clone());
        pn_table.insert(contacts[2].id().clone(), interface_three.clone());
        pn_table.insert(contacts[5].id().clone(), interface_six.clone());

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

        let hasher = Hasher::default();
        let config = PrecomputePathIdsConfig {
            vicinity_radius: 3,
            update_interval: None,
            hasher: hasher.clone(),
        };
        let mut use_case = PrecomputePathIds::new(root_id, config);
        assert!(
            use_case.start(&sync_context).is_ok(),
            "failed to start use case"
        );

        let events = [
            // Contact events to add physical neighbors
            UseCaseEvent::Contact(ContactEvent::New(contacts[1].clone())),
            UseCaseEvent::Contact(ContactEvent::New(contacts[2].clone())),
            UseCaseEvent::Contact(ContactEvent::New(contacts[5].clone())),
            // Message from Node 2 to 1 with neighbors 1 and 4
            UseCaseEvent::Message(
                ProtocolMessage::PNDiscReq(ReqRspMessage {
                    nonce: Nonce::random(),
                    source_state_seq_nr: StateSeqNr::from(2),
                    data: RTableData {
                        contacts: vec![contacts[0].clone(), contacts[3].clone()],
                    },
                    not_via: Default::default(),
                    source_route: SourceRoute::from(Path::from([
                        NodeId::with_msb(2),
                        NodeId::with_msb(1),
                    ])),
                }),
                interface_two.clone(),
            ),
            // Message from Node 3 to 1 with neighbors 1, 4 and 6
            UseCaseEvent::Message(
                ProtocolMessage::PNDiscReq(ReqRspMessage {
                    nonce: Nonce::random(),
                    source_state_seq_nr: StateSeqNr::from(3),
                    data: RTableData {
                        contacts: vec![
                            contacts[0].clone(),
                            contacts[3].clone(),
                            contacts[5].clone(),
                        ],
                    },
                    not_via: Default::default(),
                    source_route: SourceRoute::from(Path::from([
                        NodeId::with_msb(3),
                        NodeId::with_msb(1),
                    ])),
                }),
                interface_three.clone(),
            ),
            // Message from Node 6 to 1 with neighbors 1, 3 and 7
            UseCaseEvent::Message(
                ProtocolMessage::PNDiscReq(ReqRspMessage {
                    nonce: Nonce::random(),
                    source_state_seq_nr: StateSeqNr::from(6),
                    data: RTableData {
                        contacts: vec![
                            contacts[0].clone(),
                            contacts[2].clone(),
                            contacts[6].clone(),
                        ],
                    },
                    not_via: Default::default(),
                    source_route: SourceRoute::from(Path::from([
                        NodeId::with_msb(6),
                        NodeId::with_msb(1),
                    ])),
                }),
                interface_six.clone(),
            ),
            // Message from Node 4 to 1 with neighbors 2, 3 and 5
            UseCaseEvent::Message(
                ProtocolMessage::PNDiscReq(ReqRspMessage {
                    nonce: Nonce::random(),
                    source_state_seq_nr: StateSeqNr::from(4),
                    data: RTableData {
                        contacts: vec![
                            contacts[1].clone(),
                            contacts[2].clone(),
                            contacts[4].clone(),
                        ],
                    },
                    not_via: Default::default(),
                    source_route: SourceRoute::from(Path::from([
                        NodeId::with_msb(4),
                        NodeId::with_msb(2),
                        NodeId::with_msb(1),
                    ]))
                    .advanced(),
                }),
                interface_two.clone(),
            ),
            // Message from Node 7 to 1 with neighbors 6 and 5
            UseCaseEvent::Message(
                ProtocolMessage::PNDiscReq(ReqRspMessage {
                    nonce: Nonce::random(),
                    source_state_seq_nr: StateSeqNr::from(7),
                    data: RTableData {
                        contacts: vec![contacts[5].clone(), contacts[4].clone()],
                    },
                    not_via: Default::default(),
                    source_route: SourceRoute::from(Path::from([
                        NodeId::with_msb(7),
                        NodeId::with_msb(6),
                        NodeId::with_msb(1),
                    ]))
                    .advanced(),
                }),
                interface_six.clone(),
            ),
            // Message from Node 5 to 1 with neighbors 4 and 7
            UseCaseEvent::Message(
                ProtocolMessage::PNDiscReq(ReqRspMessage {
                    nonce: Nonce::random(),
                    source_state_seq_nr: StateSeqNr::from(5),
                    data: RTableData {
                        contacts: vec![contacts[3].clone(), contacts[6].clone()],
                    },
                    not_via: Default::default(),
                    source_route: SourceRoute::from(Path::from([
                        NodeId::with_msb(5),
                        NodeId::with_msb(4),
                        NodeId::with_msb(3),
                        NodeId::with_msb(1),
                    ]))
                    .advanced()
                    .advanced(),
                }),
                interface_three.clone(),
            ),
            UseCaseEvent::Contact(ContactEvent::Updated {
                new: {
                    let mut contact_three = contacts[2].clone();
                    *contact_three.state_mut() = ContactState::Invalid;
                    contact_three
                },
                old: contacts[2].clone(),
            }),
            UseCaseEvent::Contact(ContactEvent::Updated {
                old: {
                    let mut contact_three = contacts[2].clone();
                    *contact_three.state_mut() = ContactState::Invalid;
                    contact_three
                },
                new: contacts[2].clone(),
            }),
        ];

        for event in events {
            let result = use_case.handle_event(&sync_context, event);
            assert!(result.is_ok(), "Failed to handle ");
        }

        let expected_paths_with_interface = Vec::from([
            // All paths to 2
            (
                Path::from([NodeId::with_msb(1), NodeId::with_msb(2)]),
                interface_two.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(3),
                    NodeId::with_msb(4),
                    NodeId::with_msb(2),
                ]),
                interface_three.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(6),
                    NodeId::with_msb(3),
                    NodeId::with_msb(4),
                    NodeId::with_msb(2),
                ]),
                interface_six.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(6),
                    NodeId::with_msb(7),
                    NodeId::with_msb(5),
                    NodeId::with_msb(4),
                    NodeId::with_msb(2),
                ]),
                interface_six.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(3),
                    NodeId::with_msb(6),
                    NodeId::with_msb(7),
                    NodeId::with_msb(5),
                    NodeId::with_msb(4),
                    NodeId::with_msb(2),
                ]),
                interface_three.clone(),
            ),
            // All paths to 3
            (
                Path::from([NodeId::with_msb(1), NodeId::with_msb(3)]),
                interface_three.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(2),
                    NodeId::with_msb(4),
                    NodeId::with_msb(3),
                ]),
                interface_two.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(2),
                    NodeId::with_msb(4),
                    NodeId::with_msb(5),
                    NodeId::with_msb(7),
                    NodeId::with_msb(6),
                    NodeId::with_msb(3),
                ]),
                interface_two.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(6),
                    NodeId::with_msb(3),
                ]),
                interface_six.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(6),
                    NodeId::with_msb(7),
                    NodeId::with_msb(5),
                    NodeId::with_msb(4),
                    NodeId::with_msb(3),
                ]),
                interface_six.clone(),
            ),
            // All paths to 4
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(2),
                    NodeId::with_msb(4),
                ]),
                interface_two.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(3),
                    NodeId::with_msb(4),
                ]),
                interface_three.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(3),
                    NodeId::with_msb(6),
                    NodeId::with_msb(7),
                    NodeId::with_msb(5),
                    NodeId::with_msb(4),
                ]),
                interface_three.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(6),
                    NodeId::with_msb(3),
                    NodeId::with_msb(4),
                ]),
                interface_six.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(6),
                    NodeId::with_msb(7),
                    NodeId::with_msb(5),
                    NodeId::with_msb(4),
                ]),
                interface_six.clone(),
            ),
            // All paths to 5
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(2),
                    NodeId::with_msb(4),
                    NodeId::with_msb(5),
                ]),
                interface_two.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(2),
                    NodeId::with_msb(4),
                    NodeId::with_msb(3),
                    NodeId::with_msb(6),
                    NodeId::with_msb(7),
                    NodeId::with_msb(5),
                ]),
                interface_two.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(3),
                    NodeId::with_msb(4),
                    NodeId::with_msb(5),
                ]),
                interface_three.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(3),
                    NodeId::with_msb(6),
                    NodeId::with_msb(7),
                    NodeId::with_msb(5),
                ]),
                interface_three.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(6),
                    NodeId::with_msb(7),
                    NodeId::with_msb(5),
                ]),
                interface_six.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(6),
                    NodeId::with_msb(3),
                    NodeId::with_msb(4),
                    NodeId::with_msb(5),
                ]),
                interface_six.clone(),
            ),
            // All paths to 6
            (
                Path::from([NodeId::with_msb(1), NodeId::with_msb(6)]),
                interface_six.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(3),
                    NodeId::with_msb(6),
                ]),
                interface_three.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(3),
                    NodeId::with_msb(4),
                    NodeId::with_msb(5),
                    NodeId::with_msb(7),
                    NodeId::with_msb(6),
                ]),
                interface_three.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(2),
                    NodeId::with_msb(4),
                    NodeId::with_msb(3),
                    NodeId::with_msb(6),
                ]),
                interface_two.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(2),
                    NodeId::with_msb(4),
                    NodeId::with_msb(5),
                    NodeId::with_msb(7),
                    NodeId::with_msb(6),
                ]),
                interface_two.clone(),
            ),
            // All paths to 7
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(6),
                    NodeId::with_msb(7),
                ]),
                interface_six.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(6),
                    NodeId::with_msb(3),
                    NodeId::with_msb(4),
                    NodeId::with_msb(5),
                    NodeId::with_msb(7),
                ]),
                interface_six.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(3),
                    NodeId::with_msb(6),
                    NodeId::with_msb(7),
                ]),
                interface_three.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(3),
                    NodeId::with_msb(4),
                    NodeId::with_msb(5),
                    NodeId::with_msb(7),
                ]),
                interface_three.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(2),
                    NodeId::with_msb(4),
                    NodeId::with_msb(3),
                    NodeId::with_msb(6),
                    NodeId::with_msb(7),
                ]),
                interface_two.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(2),
                    NodeId::with_msb(4),
                    NodeId::with_msb(5),
                    NodeId::with_msb(7),
                ]),
                interface_two.clone(),
            ),
        ]);

        let fwd_table = sync_context.forwarding_tables();
        for (in_path, interface) in expected_paths_with_interface {
            let out_path =
                Result::<Path, EmptyPathError>::from_iter(in_path.clone().into_iter().skip(1))
                    .expect("invalid expected path");
            let in_path_id = hasher.hash(&in_path);
            // FIXME
            let entry = PathIdEntry {
                in_path_id: in_path_id.clone(),
                out_path_id: Some(hasher.hash(&out_path)),
                next_hop: out_path.first().clone(),
            };
            let created_entry = fwd_table.path_id_entry(&in_path_id);
            assert_eq!(
                created_entry,
                Some(&entry),
                "PathIdEntry for {} not created or invalid: {:?}",
                in_path,
                created_entry
            );
        }
    }

    #[test]
    fn precompute_only_paths_to_nodes_in_vicinity() {
        crate::tests::init();

        /*
        Topology:
            root_id: 1
            not in vicinity: 5

             /- 2 -\
           1 -- 3 -- 4 -- 5
                |         |
             `- 6 -- 7 -´

        */

        let interface_two = NetworkInterface::with_name("1--2");
        let interface_three = NetworkInterface::with_name("1--3");
        let interface_six = NetworkInterface::with_name("1--6");

        let root_id = NodeId::with_msb(1);

        let contacts = [
            Contact::new(Path::from(NodeId::with_msb(1)), StateSeqNr::from(1)),
            Contact::new(Path::from(NodeId::with_msb(2)), StateSeqNr::from(2)),
            Contact::new(Path::from(NodeId::with_msb(3)), StateSeqNr::from(3)),
            Contact::new(Path::from(NodeId::with_msb(4)), StateSeqNr::from(4)),
            Contact::new(Path::from(NodeId::with_msb(5)), StateSeqNr::from(5)),
            Contact::new(Path::from(NodeId::with_msb(6)), StateSeqNr::from(6)),
            Contact::new(Path::from(NodeId::with_msb(7)), StateSeqNr::from(7)),
        ];

        let (broadcaster, _broadcast_receiver) = crate::broadcaster::MPSCBroadcaster::new(30);

        let runtime = ImmediateRuntime::new(broadcaster);

        let (hub_sender, _hub_receiver) = InMemoryMessageChannel::default().into_parts();

        let mut routing_table = SingleBucketRT::<20>::new(root_id.clone());
        assert!(routing_table.insert(contacts[1].clone()).is_ok());
        assert!(routing_table.insert(contacts[2].clone()).is_ok());
        assert!(routing_table.insert(contacts[5].clone()).is_ok());

        let mut pn_table = InMemoryPNTable::new();
        pn_table.insert(contacts[1].id().clone(), interface_two.clone());
        pn_table.insert(contacts[2].id().clone(), interface_three.clone());
        pn_table.insert(contacts[5].id().clone(), interface_six.clone());

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

        let hasher = Hasher::default();
        let config = PrecomputePathIdsConfig {
            // THIS IS THE IMPORTANT POINT HERE
            vicinity_radius: 2,
            update_interval: None,
            hasher: hasher.clone(),
        };
        let mut use_case = PrecomputePathIds::new(root_id, config);
        assert!(
            use_case.start(&sync_context).is_ok(),
            "failed to start use case"
        );

        let events = [
            // Contact events to add physical neighbors
            UseCaseEvent::Contact(ContactEvent::New(contacts[1].clone())),
            UseCaseEvent::Contact(ContactEvent::New(contacts[2].clone())),
            UseCaseEvent::Contact(ContactEvent::New(contacts[5].clone())),
            // Message from Node 2 to 1 with neighbors 1 and 4
            UseCaseEvent::Message(
                ProtocolMessage::PNDiscReq(ReqRspMessage {
                    nonce: Nonce::random(),
                    source_state_seq_nr: StateSeqNr::from(2),
                    data: RTableData {
                        contacts: vec![contacts[0].clone(), contacts[3].clone()],
                    },
                    not_via: Default::default(),
                    source_route: SourceRoute::from(Path::from([
                        NodeId::with_msb(2),
                        NodeId::with_msb(1),
                    ])),
                }),
                interface_two.clone(),
            ),
            // Message from Node 3 to 1 with neighbors 1, 4 and 6
            UseCaseEvent::Message(
                ProtocolMessage::PNDiscReq(ReqRspMessage {
                    nonce: Nonce::random(),
                    source_state_seq_nr: StateSeqNr::from(3),
                    data: RTableData {
                        contacts: vec![
                            contacts[0].clone(),
                            contacts[3].clone(),
                            contacts[5].clone(),
                        ],
                    },
                    not_via: Default::default(),
                    source_route: SourceRoute::from(Path::from([
                        NodeId::with_msb(3),
                        NodeId::with_msb(1),
                    ])),
                }),
                interface_three.clone(),
            ),
            // Message from Node 6 to 1 with neighbors 1, 3 and 7
            UseCaseEvent::Message(
                ProtocolMessage::PNDiscReq(ReqRspMessage {
                    nonce: Nonce::random(),
                    source_state_seq_nr: StateSeqNr::from(6),
                    data: RTableData {
                        contacts: vec![
                            contacts[0].clone(),
                            contacts[2].clone(),
                            contacts[6].clone(),
                        ],
                    },
                    not_via: Default::default(),
                    source_route: SourceRoute::from(Path::from([
                        NodeId::with_msb(6),
                        NodeId::with_msb(1),
                    ])),
                }),
                interface_six.clone(),
            ),
            // Message from Node 4 to 1 with neighbors 2, 3 and 5
            UseCaseEvent::Message(
                ProtocolMessage::PNDiscReq(ReqRspMessage {
                    nonce: Nonce::random(),
                    source_state_seq_nr: StateSeqNr::from(4),
                    data: RTableData {
                        contacts: vec![
                            contacts[1].clone(),
                            contacts[2].clone(),
                            contacts[4].clone(),
                        ],
                    },
                    not_via: Default::default(),
                    source_route: SourceRoute::from(Path::from([
                        NodeId::with_msb(4),
                        NodeId::with_msb(2),
                        NodeId::with_msb(1),
                    ]))
                    .advanced(),
                }),
                interface_two.clone(),
            ),
            // Message from Node 7 to 1 with neighbors 6 and 5
            UseCaseEvent::Message(
                ProtocolMessage::PNDiscReq(ReqRspMessage {
                    nonce: Nonce::random(),
                    source_state_seq_nr: StateSeqNr::from(7),
                    data: RTableData {
                        contacts: vec![contacts[5].clone(), contacts[4].clone()],
                    },
                    not_via: Default::default(),
                    source_route: SourceRoute::from(Path::from([
                        NodeId::with_msb(7),
                        NodeId::with_msb(6),
                        NodeId::with_msb(1),
                    ]))
                    .advanced(),
                }),
                interface_six.clone(),
            ),
            // Message from Node 5 to 1 with neighbors 4 and 7
            UseCaseEvent::Message(
                ProtocolMessage::PNDiscReq(ReqRspMessage {
                    nonce: Nonce::random(),
                    source_state_seq_nr: StateSeqNr::from(5),
                    data: RTableData {
                        contacts: vec![contacts[3].clone(), contacts[6].clone()],
                    },
                    not_via: Default::default(),
                    source_route: SourceRoute::from(Path::from([
                        NodeId::with_msb(5),
                        NodeId::with_msb(4),
                        NodeId::with_msb(3),
                        NodeId::with_msb(1),
                    ]))
                    .advanced()
                    .advanced(),
                }),
                interface_three.clone(),
            ),
        ];

        for event in events {
            let result = use_case.handle_event(&sync_context, event);
            assert!(result.is_ok(), "Failed to handle ");
        }

        let expected_paths_with_interface = Vec::from([
            // All paths to 2
            (
                Path::from([NodeId::with_msb(1), NodeId::with_msb(2)]),
                interface_two.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(3),
                    NodeId::with_msb(4),
                    NodeId::with_msb(2),
                ]),
                interface_three.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(6),
                    NodeId::with_msb(3),
                    NodeId::with_msb(4),
                    NodeId::with_msb(2),
                ]),
                interface_six.clone(),
            ),
            // All paths to 3
            (
                Path::from([NodeId::with_msb(1), NodeId::with_msb(3)]),
                interface_three.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(2),
                    NodeId::with_msb(4),
                    NodeId::with_msb(3),
                ]),
                interface_two.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(6),
                    NodeId::with_msb(3),
                ]),
                interface_six.clone(),
            ),
            // All paths to 4
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(2),
                    NodeId::with_msb(4),
                ]),
                interface_two.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(3),
                    NodeId::with_msb(4),
                ]),
                interface_three.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(6),
                    NodeId::with_msb(3),
                    NodeId::with_msb(4),
                ]),
                interface_six.clone(),
            ),
            // All paths to 6
            (
                Path::from([NodeId::with_msb(1), NodeId::with_msb(6)]),
                interface_six.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(3),
                    NodeId::with_msb(6),
                ]),
                interface_three.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(2),
                    NodeId::with_msb(4),
                    NodeId::with_msb(3),
                    NodeId::with_msb(6),
                ]),
                interface_two.clone(),
            ),
            // All paths to 7
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(6),
                    NodeId::with_msb(7),
                ]),
                interface_six.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(3),
                    NodeId::with_msb(6),
                    NodeId::with_msb(7),
                ]),
                interface_three.clone(),
            ),
            (
                Path::from([
                    NodeId::with_msb(1),
                    NodeId::with_msb(2),
                    NodeId::with_msb(4),
                    NodeId::with_msb(3),
                    NodeId::with_msb(6),
                    NodeId::with_msb(7),
                ]),
                interface_two.clone(),
            ),
        ]);

        let fwd_table = sync_context.forwarding_tables();
        for (in_path, interface) in expected_paths_with_interface {
            let out_path =
                Result::<Path, EmptyPathError>::from_iter(in_path.clone().into_iter().skip(1))
                    .expect("invalid expected path");
            let in_path_id = hasher.hash(&in_path);
            let entry = PathIdEntry {
                in_path_id: in_path_id.clone(),
                out_path_id: Some(hasher.hash(&out_path)),
                next_hop: out_path.first().clone(),
            };
            let created_entry = fwd_table.path_id_entry(&in_path_id);
            assert_eq!(
                created_entry,
                Some(&entry),
                "PathIdEntry for {} not created or invalid: {:?}",
                in_path,
                created_entry
            );
        }
    }
}
