//! A [VicinityGraph] implementation supported by [petgraph].

use std::{
    collections::{
        HashMap, HashSet,
        hash_map::Entry::{Occupied, Vacant},
    },
    hash::RandomState,
    time::Instant,
};

use derive_more::derive::{Display, Error};
use petgraph::{
    algo::{all_simple_paths, dijkstra},
    prelude::UnGraphMap,
};

use super::VicinityGraph;
use crate::domain::{NodeId, Path, SafeStateSeqNr, VICINITY_RADIUS};

#[derive(Debug, Eq, PartialEq, Clone)]
/// An [Entry] of the [VicinityGraph].
pub struct Entry {
    last_seen: Instant,
    /// State sequence number of the vicinity present in the [VicinityGraph].
    ///
    /// The value can be [None] before the initial synchronization is completed.
    vicinity_ssn: Option<SafeStateSeqNr>,
    /// Greatest known state sequence number.
    known_ssn: SafeStateSeqNr,
}

/// Graph which generates all [Path]s from a given root [NodeId].
#[derive(Debug, Clone)]
pub struct PetVicinityGraph {
    root_id: NodeId,
    graph: UnGraphMap<NodeId, ()>,
    entries: HashMap<NodeId, Entry>,
}

/// Errors that can happen on [VicinityGraph::insert] or [VicinityGraph::add]
#[derive(Debug, Display, Error)]
pub enum VicinityError {
    #[display("No neighbor of {node} inside the VicinityGraph: {neighbors:#?}")]
    NeighborNotFound {
        #[error(ignore)]
        node: NodeId,
        #[error(ignore)]
        neighbors: HashSet<NodeId>,
    },
    #[display("{provided} is older than expected: {expected}")]
    OldSSSN {
        #[error(ignore)]
        expected: SafeStateSeqNr,
        #[error(ignore)]
        provided: SafeStateSeqNr,
    },
}

impl PetVicinityGraph {
    /// Create a new VicinityGraph.
    pub fn new(root_id: NodeId) -> Self {
        let mut graph = UnGraphMap::new();
        graph.add_node(root_id);
        Self {
            root_id,
            graph,
            entries: HashMap::default(),
        }
    }
}

impl PetVicinityGraph {
    fn valid_ssn(&self, node: &NodeId, ssn: &SafeStateSeqNr) -> Result<(), VicinityError> {
        let Some(Entry {
            vicinity_ssn: Some(vicinity_ssn),
            ..
        }) = self.entries.get(node)
        else {
            return Ok(());
        };
        if ssn <= vicinity_ssn {
            Err(VicinityError::OldSSSN {
                expected: (*vicinity_ssn + 1)
                    .value()
                    .unwrap_or(SafeStateSeqNr::try_from(1).unwrap()),
                provided: *ssn,
            })
        } else {
            Ok(())
        }
    }

    fn valid_neighbors(
        &self,
        node: &NodeId,
        neighbors: impl IntoIterator<Item = NodeId>,
    ) -> Result<HashSet<NodeId>, VicinityError> {
        let neighbors: HashSet<_> = neighbors.into_iter().collect();
        // guard against unconnected nodes
        // and nodes that are not connected to entries
        if neighbors
            .iter()
            .any(|neighbor| neighbor == &self.root_id || self.contains(neighbor))
        {
            Ok(neighbors)
        } else {
            Err(VicinityError::NeighborNotFound {
                node: *node,
                neighbors,
            })
        }
    }

    fn contains(&self, node: &NodeId) -> bool {
        self.entries.contains_key(node) && self.graph.contains_node(*node)
    }
}

impl VicinityGraph for PetVicinityGraph {
    type Error = VicinityError;

    fn insert(
        &mut self,
        node: NodeId,
        neighbors: impl IntoIterator<Item = NodeId>,
        ssn: SafeStateSeqNr,
        last_seen: Instant,
    ) -> Result<(), Self::Error> {
        let neighbors = self.valid_neighbors(&node, neighbors)?;
        self.valid_ssn(&node, &ssn)?;

        if let Some(entry) = self.entries.insert(
            node,
            Entry {
                last_seen,
                vicinity_ssn: Some(ssn),
                known_ssn: ssn,
            },
        ) {
            assert!(last_seen >= entry.last_seen, "don't go backwards in time");
        }
        self.graph.remove_node(node); // remove all previous links to/from node
        for neighbor in neighbors {
            self.graph.add_edge(node, neighbor, ());
        }

        Ok(())
    }

    fn remove(&mut self, node: &NodeId) -> bool {
        if node == &self.root_id {
            return false;
        }

        // keep entry to resync up to known_ssn after reconnect
        if let Some(entry) = self.entries.get_mut(node) {
            entry.vicinity_ssn = None;
        }
        self.graph.remove_node(*node)
    }

    fn prune(&mut self, node: &NodeId) -> bool {
        if node == &self.root_id {
            return false;
        }

        self.entries.remove(node);
        self.graph.remove_node(*node)
    }

    fn remove_radius(&mut self) -> bool {
        let dist = dijkstra(&self.graph, self.root_id, None, |_| 1);

        let mut removed = false;
        for (node, dist) in dist.iter() {
            if dist >= &VICINITY_RADIUS {
                removed |= self.graph.remove_node(*node);
            }
        }

        let unconnected: Vec<_> = self
            .graph
            .nodes()
            .filter(|n| dist.get(n).is_none())
            .collect();

        // remove all unconnected nodes
        for node in unconnected {
            removed |= self.graph.remove_node(node);
        }
        removed
    }

    fn nodes(&self) -> impl Iterator<Item = &NodeId> {
        self.entries.keys()
    }

    fn paths(&self) -> impl Iterator<Item = Path> {
        self.nodes()
            .flat_map(|node| {
                all_simple_paths::<_, _, RandomState>(
                    &self.graph,
                    self.root_id,
                    *node,
                    // intermediate nodes: start and end node excluded
                    0,
                    Some(const { VICINITY_RADIUS - 2 }),
                )
            })
            .filter_map(Result::ok)
    }

    fn paths_to(&self, destination: NodeId) -> impl Iterator<Item = Path> {
        // TODO: check if a path exists to avoid potential long runtime on large vicinity
        all_simple_paths::<_, _, RandomState>(
            &self.graph,
            self.root_id,
            destination,
            0,
            Some(const { VICINITY_RADIUS - 2 }),
        )
        .filter_map(Result::ok)
    }

    fn update_last_seen(&mut self, node: &NodeId, now: Instant) -> bool {
        let Some(entry) = self.entries.get_mut(node) else {
            return false;
        };
        assert!(now >= entry.last_seen, "don't go backwards in time");
        entry.last_seen = now;
        true
    }

    fn last_seen(&self, node: &NodeId) -> Option<Instant> {
        self.entries.get(node).map(|e| e.last_seen)
    }

    fn update_ssn(&mut self, node: NodeId, ssn: SafeStateSeqNr, now: Instant) -> bool {
        match self.entries.entry(node) {
            Occupied(mut entry) => {
                let entry = entry.get_mut();
                assert!(now >= entry.last_seen, "don't go backwards in time");

                entry.last_seen = now;
                if entry.known_ssn < ssn {
                    entry.known_ssn = ssn;
                    true
                } else {
                    false
                }
            }
            Vacant(entry) => {
                entry.insert(Entry {
                    last_seen: now,
                    vicinity_ssn: None,
                    known_ssn: ssn,
                });
                true
            }
        }
    }

    fn ssn_vicinity(&self, node: &NodeId) -> Option<SafeStateSeqNr> {
        self.entries.get(node).and_then(|entry| entry.vicinity_ssn)
    }

    fn force_resync(&mut self, node: &NodeId) {
        if let Some(entry) = self.entries.get_mut(node) {
            entry.vicinity_ssn = None;
            entry.known_ssn = SafeStateSeqNr::try_from(1).unwrap();
        }
    }

    fn requires_resync(&self, node: &NodeId) -> bool {
        // after removal don't require resync
        if !self.contains(node) {
            return false;
        }

        self.entries.get(node).is_some_and(|entry| {
            entry
                .vicinity_ssn
                .is_none_or(|vicinity_ssn| vicinity_ssn < entry.known_ssn)
        })
    }

    // FIXME: method seems broken
    /*fn resync_nodes(&self) -> impl Iterator<Item = &NodeId> {
        self.entries
            .iter()
            .filter(|(_, entry)| {
                entry
                    .vicinity_ssn
                    .is_none_or(|vicinity_ssn| vicinity_ssn < entry.known_ssn)
            })
            .map(|(nid, _)| nid)
    }*/
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn no_entry_for_root_id() {
        let root_id = NodeId::with_msb(1);

        let graph = PetVicinityGraph::new(root_id);
        assert_eq!(graph.entries.get(&root_id), None);

        assert_eq!(
            graph.nodes().collect::<Vec<_>>().len(),
            0,
            "root is not part of the vicinity"
        );
    }

    #[test]
    fn no_insertion_of_root_id() {
        let root_id = NodeId::with_msb(1);

        let mut graph = PetVicinityGraph::new(root_id);
        assert_eq!(graph.entries.get(&root_id), None);

        assert!(
            matches!(
                graph.insert(
                    root_id,
                    vec![NodeId::with_msb(2)],
                    SafeStateSeqNr::try_from(1).unwrap(),
                    Instant::now(),
                ),
                Err(VicinityError::NeighborNotFound { .. })
            ),
            "root can't be inserted in the vicinity"
        );
    }

    #[test]
    fn insert() {
        let root_id = NodeId::with_msb(1);
        let node_to_insert = NodeId::with_msb(2);
        let vicinity_neighbor = NodeId::with_msb(3);
        let ssn = SafeStateSeqNr::try_from(2).unwrap();

        let paths_to_node = vec![Path::try_from(vec![root_id, node_to_insert]).unwrap()];
        let paths_to_vicinity_neighbor =
            vec![Path::try_from(vec![root_id, node_to_insert, vicinity_neighbor]).unwrap()];

        let mut graph = PetVicinityGraph::new(root_id);

        assert!(
            graph
                .insert(
                    node_to_insert,
                    vec![root_id, vicinity_neighbor],
                    ssn,
                    Instant::now()
                )
                .is_ok(),
            "failed to insert NodeId"
        );
        let nodes: HashSet<_> = graph.nodes().collect();
        let paths: HashSet<_> = graph.paths().collect();

        // test node to insert
        assert!(
            graph.last_seen(&node_to_insert).is_some(),
            "should have been seen"
        );
        assert!(
            !graph.requires_resync(&node_to_insert),
            "shouldn't trigger resync"
        );
        assert_eq!(
            graph.ssn_vicinity(&node_to_insert),
            Some(ssn),
            "state of vicinity should be present after insertion"
        );
        assert!(
            nodes.contains(&node_to_insert),
            "nodes of the vicinity graph should contain the vicinity neighbor"
        );
        assert_eq!(
            graph.paths_to(node_to_insert).collect::<Vec<_>>(),
            paths_to_node,
            "should generate all paths to node"
        );
        assert!(
            HashSet::from_iter(paths_to_node.into_iter())
                .difference(&paths)
                .next()
                .is_none(),
            "should generate all paths to node"
        );

        // test vicinity neighbor
        assert!(
            graph.last_seen(&vicinity_neighbor).is_none(),
            "vicinity neighbor shouldn't have been seen"
        );
        assert!(
            !graph.requires_resync(&vicinity_neighbor),
            "vicinity neighbor should be marked for resync" // since it's in the vicinity radius
        );
        assert_eq!(
            graph.ssn_vicinity(&vicinity_neighbor),
            None,
            "shouldn't be a vicinity information about the vicinity neighbor"
        );
        assert!(
            nodes.contains(&vicinity_neighbor),
            "nodes of the vicinity graph should contain the vicinity neighbor"
        );
        assert_eq!(
            graph.paths_to(vicinity_neighbor).collect::<Vec<_>>(),
            paths_to_vicinity_neighbor,
            "should generate all paths to vicinity neighbor"
        );
        assert!(
            HashSet::from_iter(paths_to_vicinity_neighbor.into_iter())
                .difference(&paths)
                .next()
                .is_none(),
            "should generate all paths to vicinity neighbor"
        );
    }

    #[test]
    fn insert_updated_vicinity() {
        // tests if the link to the neighbor is removed after successful insertion
        /*
        Initial Topology:
                4
                |
             /- 2 -\
            1 ----- 3

        Topology update driven by 2:
                4
                |
             /- 2
            1 ----- 3
        */
        let root_id = NodeId::with_msb(1);
        let node_2 = NodeId::with_msb(2);
        let node_3 = NodeId::with_msb(3);
        let node_4 = NodeId::with_msb(4);

        let mut graph = PetVicinityGraph::new(root_id);

        // initial topology
        assert!(
            graph
                .insert(
                    node_2,
                    vec![root_id, node_3, node_4],
                    SafeStateSeqNr::try_from(3).unwrap(),
                    Instant::now()
                )
                .is_ok(),
            "failed to insert NodeId"
        );
        assert!(
            graph
                .insert(
                    node_3,
                    vec![root_id, node_2],
                    SafeStateSeqNr::try_from(2).unwrap(),
                    Instant::now()
                )
                .is_ok(),
            "failed to insert NodeId"
        );

        assert!(
            graph
                .insert(
                    node_4,
                    vec![node_2],
                    SafeStateSeqNr::try_from(2).unwrap(),
                    Instant::now()
                )
                .is_ok(),
            "failed to insert NodeId"
        );

        assert!(
            graph.resync_nodes().next().is_none(),
            "no nodes needs to be resynchronised"
        );

        // link between n2 and n3 fail
        assert!(
            graph
                .insert(
                    node_2,
                    vec![root_id, node_4],
                    SafeStateSeqNr::try_from(4).unwrap(),
                    Instant::now()
                )
                .is_ok(),
            "failed to insert NodeId"
        );

        // check links
        assert!(
            !graph.graph.contains_edge(node_2, node_3),
            "link got deleted"
        );
        assert!(
            graph.graph.contains_edge(node_2, node_4),
            "link still present"
        );
        assert!(
            graph.graph.contains_edge(node_2, root_id),
            "link still present"
        );
        assert!(
            graph.graph.contains_edge(node_3, root_id),
            "link still present"
        );

        assert!(
            graph.requires_resync(&node_3),
            "should resync with node 3 after link failure"
        );
    }

    #[test]
    fn reject_unconnected_insertion() {
        let root_id = NodeId::with_msb(1);
        let mut graph = PetVicinityGraph::new(root_id);

        assert!(
            matches!(
                graph.insert(
                    NodeId::with_msb(2),
                    None,
                    SafeStateSeqNr::try_from(1).unwrap(),
                    Instant::now(),
                ),
                Err(VicinityError::NeighborNotFound { .. })
            ),
            "shouldn't insert unconnected node"
        );

        assert!(
            matches!(
                graph.insert(
                    NodeId::with_msb(2),
                    vec![NodeId::with_msb(4)],
                    SafeStateSeqNr::try_from(1).unwrap(),
                    Instant::now(),
                ),
                Err(VicinityError::NeighborNotFound { .. })
            ),
            "shouldn't insert unconnected node"
        );
    }

    #[test]
    fn reject_unconnected_insertion_over_unconfirmed_entry() {
        /*
        Topology:
            1 - 2 - (3) - 4

        Note:
          This case can't really be happening because the entry would be out
          of the vicinity radius but still we better prevent it,
          if we doctor with the vicinity radius.
        */

        let root_id = NodeId::with_msb(1);

        let mut graph = PetVicinityGraph::new(root_id);

        assert!(
            graph
                .insert(
                    NodeId::with_msb(2),
                    vec![root_id, NodeId::with_msb(3)],
                    SafeStateSeqNr::try_from(1).unwrap(),
                    Instant::now(),
                )
                .is_ok(),
            "should insert node connected to root",
        );

        assert!(
            graph.contains(&NodeId::with_msb(2)),
            "should contain NodeId(2) after insertion"
        );
        assert!(
            graph.nodes().any(|n| n == &NodeId::with_msb(2)),
            "should contain NodeId(2) after insertion"
        );

        assert!(
            matches!(
                graph.insert(
                    NodeId::with_msb(4),
                    vec![NodeId::with_msb(3)], // connected via NodeId(3) but not yet confirmed
                    SafeStateSeqNr::try_from(1).unwrap(),
                    Instant::now(),
                ),
                Err(VicinityError::NeighborNotFound { .. }),
            ),
            "shouldn't insert node connected via unconfirmed NodeId(3)"
        );
    }

    #[test]
    fn remove() {
        let root_id = NodeId::with_msb(1);
        let node_to_remove = NodeId::with_msb(2);
        let ssn = SafeStateSeqNr::try_from(2).unwrap();

        let mut graph = PetVicinityGraph::new(root_id);

        assert!(
            graph
                .insert(node_to_remove, vec![root_id], ssn, Instant::now())
                .is_ok(),
            "failed to insert NodeId"
        );
        assert!(graph.remove(&node_to_remove), "failed to remove NodeId");

        assert!(
            !graph.contains(&node_to_remove),
            "shouldn't contain node after removal"
        );
        assert!(
            graph.last_seen(&node_to_remove).is_some(),
            "should still have been seen"
        );
        assert!(
            !graph.requires_resync(&node_to_remove),
            "shouldn't trigger resync"
        );
        assert!(
            graph.ssn_vicinity(&node_to_remove).is_none(),
            "information about vicinity after removal should be gone"
        );
    }

    #[test]
    fn remove_vicinity() {
        // tests if vicinity of the removed node is marked for resynchronisation

        /*
        Topology:
                4
                |
             /- 2 -\
            1 ----- 3

        Topology after Removal:
                4 # the node is going to be removed on resync attempts because it is unreachable

            1 ----- 3
        */

        let root_id = NodeId::with_msb(1);
        let node_2 = NodeId::with_msb(2);
        let node_3 = NodeId::with_msb(3);
        let node_4 = NodeId::with_msb(4);
        let ssn = SafeStateSeqNr::try_from(2).unwrap();

        let mut graph = PetVicinityGraph::new(root_id);

        assert!(
            graph
                .insert(node_2, vec![root_id, node_3, node_4], ssn, Instant::now())
                .is_ok(),
            "failed to insert NodeId"
        );
        assert!(
            graph
                .insert(node_3, vec![root_id, node_2], ssn, Instant::now())
                .is_ok(),
            "failed to insert NodeId"
        );
        assert!(graph.remove(&node_2), "failed to remove NodeId");

        // we don't proactively resync neighbors
        // we pick up the changed SSN in the next received ULNHello
        assert!(
            !graph.requires_resync(&node_3),
            "shouldn't require resync after vicinity changed"
        );
        assert!(
            graph.requires_resync(&node_4),
            "shouldn't require resync after vicinity changed"
        );
        assert!(
            graph.graph.contains_edge(root_id, node_3),
            "should contain edge to node 3"
        );
    }

    #[test]
    fn resync_after_remove() {
        // tests if the resync only stops after last known ssn was reached after removal

        let root_id = NodeId::with_msb(1);
        let node_to_remove = NodeId::with_msb(2);
        let outdated_ssn_1 = SafeStateSeqNr::try_from(1).unwrap();
        let outdated_ssn_2 = SafeStateSeqNr::try_from(2).unwrap();
        let ssn = SafeStateSeqNr::try_from(3).unwrap();

        let mut graph = PetVicinityGraph::new(root_id);

        assert!(
            graph
                .insert(
                    node_to_remove,
                    vec![root_id, NodeId::with_msb(3), NodeId::with_msb(4)],
                    ssn,
                    Instant::now()
                )
                .is_ok(),
            "failed to insert NodeId"
        );
        assert!(graph.remove(&node_to_remove), "failed to remove NodeId");

        assert!(
            graph
                .insert(
                    node_to_remove,
                    vec![root_id],
                    outdated_ssn_1,
                    Instant::now()
                )
                .is_ok(),
            "failed to insert node with outdated data" // at least something
        );
        assert!(
            !graph.requires_resync(&node_to_remove),
            "should require resync" // since we still know the last seen ssn
        );
        assert!(
            graph
                .insert(
                    node_to_remove,
                    vec![root_id, NodeId::with_msb(3)],
                    outdated_ssn_2,
                    Instant::now()
                )
                .is_ok(),
            "failed to insert node with outdated data"
        );
        assert!(
            !graph.requires_resync(&node_to_remove),
            "should require resync"
        );

        assert!(
            graph
                .insert(
                    node_to_remove,
                    vec![root_id, NodeId::with_msb(3), NodeId::with_msb(4)],
                    ssn,
                    Instant::now()
                )
                .is_ok(),
            "failed to insert node with previous state"
        );
        assert!(
            !graph.requires_resync(&node_to_remove),
            "should be fully synced after reinsertion"
        );

        assert!(
            graph
                .insert(
                    node_to_remove,
                    vec![root_id],
                    SafeStateSeqNr::try_from(42).unwrap(),
                    Instant::now()
                )
                .is_ok(),
            "failed to insert node with new state"
        );
        assert!(
            !graph.requires_resync(&node_to_remove),
            "should be fully synced"
        );
    }

    #[test]
    fn prune() {
        let root_id = NodeId::with_msb(1);
        let node_to_prune = NodeId::with_msb(2);
        let ssn = SafeStateSeqNr::try_from(2).unwrap();

        let mut graph = PetVicinityGraph::new(root_id);

        assert!(
            graph
                .insert(
                    node_to_prune,
                    vec![root_id, NodeId::with_msb(3)],
                    ssn,
                    Instant::now()
                )
                .is_ok(),
            "Failed to insert NodeId"
        );
        assert!(graph.prune(&node_to_prune), "Failed to prune NodeId");

        assert!(
            !graph.contains(&node_to_prune),
            "shouldn't contain node after prune"
        );
        assert!(
            graph.last_seen(&node_to_prune).is_none(),
            "no information about last seen"
        );
        assert!(
            !graph.requires_resync(&node_to_prune),
            "shouldn't trigger resync"
        );
        assert!(
            graph.ssn_vicinity(&node_to_prune).is_none(),
            "information about vicinity after prune should be gone"
        );
    }

    #[test]
    fn outdated_insertion_after_prune() {
        // tests if we can (correctly) insert outdated vicinity data after prune

        let root_id = NodeId::with_msb(1);
        let node_to_prune = NodeId::with_msb(2);
        let outdated_ssn = SafeStateSeqNr::try_from(1).unwrap();
        let ssn = SafeStateSeqNr::try_from(2).unwrap();

        let mut graph = PetVicinityGraph::new(root_id);

        assert!(
            graph
                .insert(
                    node_to_prune,
                    vec![root_id, NodeId::with_msb(3)],
                    ssn,
                    Instant::now()
                )
                .is_ok(),
            "Failed to insert NodeId"
        );
        assert!(graph.prune(&node_to_prune), "Failed to prune NodeId");

        // now we should be able to insert even outdated data
        // this behaviour is important after a SSN-reset so we accept new
        // data that only looks outdated because

        assert!(
            graph
                .insert(
                    node_to_prune,
                    vec![root_id, NodeId::with_msb(3)],
                    outdated_ssn,
                    Instant::now()
                )
                .is_ok(),
            "failed to insert node with \"outdated\" data",
        );

        assert!(
            !graph.requires_resync(&node_to_prune),
            "shouldn't require resync after insertion of \"outdated\" data"
        );
    }

    #[test]
    fn reject_outdated_node_data() {
        let root_id = NodeId::with_msb(1);
        let old_ssn = SafeStateSeqNr::try_from(1).unwrap();
        let new_ssn = SafeStateSeqNr::try_from(42).unwrap();
        let expected_ssn = SafeStateSeqNr::try_from(43).unwrap();

        let mut graph = PetVicinityGraph::new(root_id);

        assert!(
            graph
                .insert(
                    NodeId::with_msb(2),
                    vec![root_id, NodeId::with_msb(3)],
                    new_ssn,
                    Instant::now()
                )
                .is_ok(),
            "Failed to insert NodeId"
        );

        assert!(matches!(
            graph.insert(NodeId::with_msb(2), vec![root_id], old_ssn, Instant::now(),),
            Err(VicinityError::OldSSSN {
                expected,
                ..
            }) if expected == expected_ssn
        ));
    }

    #[test]
    fn calculates_all_paths_in_2_hop_vicinity() {
        /*
        Topology:

            /- 2 -\
           1 - 3 - 4 - 5
           |   |       |
           \-- 6 -- 7 -/

        */

        let root_id = NodeId::with_msb(1);
        let now = Instant::now();
        let ssn = SafeStateSeqNr::try_from(1).unwrap();

        let mut graph = PetVicinityGraph::new(root_id);
        assert!(
            graph
                .insert(
                    NodeId::with_msb(2),
                    [NodeId::with_msb(1), NodeId::with_msb(4)],
                    ssn,
                    now,
                )
                .is_ok()
        );
        assert!(
            graph
                .insert(
                    NodeId::with_msb(3),
                    [
                        NodeId::with_msb(1),
                        NodeId::with_msb(4),
                        NodeId::with_msb(6),
                    ],
                    ssn,
                    now,
                )
                .is_ok()
        );
        assert!(
            graph
                .insert(
                    NodeId::with_msb(4),
                    [
                        NodeId::with_msb(2),
                        NodeId::with_msb(3),
                        NodeId::with_msb(5),
                    ],
                    ssn,
                    now,
                )
                .is_ok()
        );
        assert!(
            graph
                .insert(
                    NodeId::with_msb(5),
                    [NodeId::with_msb(4), NodeId::with_msb(7)],
                    ssn,
                    now,
                )
                .is_ok()
        );
        assert!(
            graph
                .insert(
                    NodeId::with_msb(6),
                    [
                        NodeId::with_msb(1),
                        NodeId::with_msb(3),
                        NodeId::with_msb(7),
                    ],
                    ssn,
                    now,
                )
                .is_ok()
        );
        assert!(
            graph
                .insert(
                    NodeId::with_msb(7),
                    [NodeId::with_msb(6), NodeId::with_msb(5)],
                    ssn,
                    now,
                )
                .is_ok()
        );

        let mut paths = graph.paths().collect::<HashSet<_>>();
        let expected_paths = HashSet::from([
            // All paths to 2
            Path::from([NodeId::with_msb(1), NodeId::with_msb(2)]),
            // All paths to 3
            Path::from([NodeId::with_msb(1), NodeId::with_msb(3)]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(6),
                NodeId::with_msb(3),
            ]),
            // All paths to 4
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(2),
                NodeId::with_msb(4),
            ]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(3),
                NodeId::with_msb(4),
            ]),
            // All paths to 5
            // NONE
            // All paths to 6
            Path::from([NodeId::with_msb(1), NodeId::with_msb(6)]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(3),
                NodeId::with_msb(6),
            ]),
            // All paths to 7
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(6),
                NodeId::with_msb(7),
            ]),
        ]);
        let mut not_generated = HashSet::new();
        for path in expected_paths {
            if !paths.remove(&path) {
                not_generated.insert(path);
            }
        }
        assert!(
            not_generated.is_empty() && paths.is_empty(),
            "Not generated paths: {not_generated:#?}; Additionally generated paths: {paths:#?}"
        );
    }

    #[test]
    fn calculates_non_bidirectional_paths() {
        // since adopting a neighbor always requires a 2-way handshake
        // the connection has to be bidirectional even though we didn't (yet)
        // get the confirming vicinity update from the other node.

        let root_id = NodeId::with_msb(1);
        let now = Instant::now();
        let ssn = SafeStateSeqNr::try_from(1).unwrap();

        let mut graph = PetVicinityGraph::new(root_id);
        assert!(
            graph
                .insert(
                    NodeId::with_msb(2),
                    [NodeId::with_msb(1), NodeId::with_msb(3)],
                    ssn,
                    now,
                )
                .is_ok(),
        );

        let mut paths = graph.paths().collect::<HashSet<_>>();
        let expected_paths = HashSet::from([
            // All paths to 2
            Path::from([NodeId::with_msb(1), NodeId::with_msb(2)]),
            // All paths to 3
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(2),
                NodeId::with_msb(3),
            ]),
        ]);
        let mut not_generated = HashSet::new();
        for path in expected_paths {
            if !paths.remove(&path) {
                not_generated.insert(path);
            }
        }
        assert!(
            not_generated.is_empty() && paths.is_empty(),
            "Not generated paths: {not_generated:#?}; Additionally generated paths: {paths:#?}"
        );

        let nodes: HashSet<_> = graph.nodes().collect();
        assert_eq!(
            nodes,
            [NodeId::with_msb(2), NodeId::with_msb(3)].iter().collect(),
            "Not all nodes present"
        );
    }

    #[test]
    fn ssn_update() {
        let root_id = NodeId::with_msb(1);
        let uln = NodeId::with_msb(2);

        let now = Instant::now();
        let outdated_ssn = SafeStateSeqNr::try_from(1).unwrap();
        let initial_ssn = SafeStateSeqNr::try_from(2).unwrap();
        let inter_ssn = SafeStateSeqNr::try_from(21).unwrap();
        let known_ssn = SafeStateSeqNr::try_from(42).unwrap();

        let mut graph = PetVicinityGraph::new(root_id);
        assert!(
            graph.insert(uln, [root_id], initial_ssn, now).is_ok(),
            "insertion of uln failed"
        );
        assert!(
            graph
                .insert(uln, [root_id, NodeId::with_msb(42)], outdated_ssn, now)
                .is_err(),
            "outdated ssn should be ignored"
        );
        assert!(
            graph.insert(uln, [root_id], initial_ssn, now).is_err(),
            "already known state should be ignored"
        );
        assert!(
            graph
                .insert(uln, [root_id, NodeId::with_msb(33)], initial_ssn, now)
                .is_err(),
            "already known state should be ignored even if something changed for some reason" // this shouldn't happen in practice but still be rejected if
        );

        assert!(
            graph.update_ssn(uln, inter_ssn, now),
            "pickup newer ssn failed"
        );
        assert!(
            graph.requires_resync(&uln),
            "should require resync after new ssn was observed"
        );

        // TODO:
        //   we need the ability to set a ssn < last known ssn if req/rsp tells us so
        //   and react "accordingly"
        assert!(
            !graph.update_ssn(uln, initial_ssn, now),
            "shouldn't update to older ssn"
        );

        assert!(
            graph.update_ssn(uln, known_ssn, now),
            "pickup new ssn failed"
        );
        assert!(
            graph.requires_resync(&uln),
            "should require resync after new ssn was observed"
        );

        assert!(
            !graph.update_ssn(uln, known_ssn, now),
            "already known ssn shouldn't be updated"
        );

        // check RESYNCHRONISATION
        assert_eq!(
            graph.resync_nodes().collect::<Vec<_>>(),
            vec![&uln],
            "uln should be resynced"
        );

        // check INSERT
        assert!(
            graph
                .insert(uln, [root_id, NodeId::with_msb(42)], outdated_ssn, now)
                .is_err(),
            "outdated ssn should be ignored"
        );
        assert!(
            graph.insert(uln, [root_id], initial_ssn, now).is_err(),
            "already known state should be ignored"
        );
        assert!(
            graph
                .insert(uln, [root_id, NodeId::with_msb(42)], inter_ssn, now)
                .is_ok(),
            "intermediat update should not be ignored" // because its at least an update
        );

        // check RESYNCHRONISATION
        assert_eq!(
            graph.resync_nodes().collect::<Vec<_>>(),
            vec![&uln],
            "uln should be resynced"
        );

        // check INSERT
        assert!(
            graph.insert(uln, [root_id], known_ssn, now).is_ok(),
            "update should not be ignored"
        );
        // check RESYNCHRONISATION
        assert!(
            graph.resync_nodes().next().is_none(),
            "uln should now be synced"
        );
    }
}
