//! A [VicinityGraph] implementation supported by [petgraph].

use std::{collections::HashMap, hash::RandomState};

use derive_more::derive::{Display, Error};
use petgraph::{
    algo::{all_simple_paths, astar, dijkstra},
    prelude::UnGraphMap,
};

use super::{Entry, VicinityGraph};
use crate::domain::{NodeId, Path, SafeStateSeqNr, VICINITY_RADIUS};

/// Graph which generates all [Path]s from a given root [NodeId].
#[derive(Debug, Clone)]
pub struct PetVicinityGraph {
    root_id: NodeId,
    graph: UnGraphMap<NodeId, ()>,   // connectivity
    entries: HashMap<NodeId, Entry>, // meta-data
}

/// Errors that can happen on [VicinityGraph::insert] or [VicinityGraph::add]
#[derive(Debug, Display, Error)]
#[cfg_attr(test, derive(PartialEq))]
pub enum PetVicinityGraphError {
    #[display("Neighbor nof {node} not in the VicinityGraph: {neighbor}")]
    NeighborNotInVicinityGraph {
        #[error(ignore)]
        node: NodeId,
        #[error(ignore)]
        neighbor: NodeId,
    },
    #[display("Tried to insert root into the VicinityGraph")]
    Root,
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
    fn valid_neighbor(
        &self,
        node: &NodeId,
        neighbor: &NodeId,
    ) -> Result<(), PetVicinityGraphError> {
        // guard against unconnected nodes
        // and nodes that are not connected to contacted nodes
        if neighbor == &self.root_id || self.entries.contains_key(neighbor) {
            Ok(())
        } else {
            Err(PetVicinityGraphError::NeighborNotInVicinityGraph {
                node: *node,
                neighbor: *neighbor,
            })
        }
    }
}

// intermediate nodes: start and end node excluded
const MAX_INTERMEDIATE_NODES: Option<usize> = Some(VICINITY_RADIUS - 2);

impl VicinityGraph for PetVicinityGraph {
    type Error = PetVicinityGraphError;

    fn insert(
        &mut self,
        node: NodeId,
        discovered_via: &NodeId,
        observed_ssn: SafeStateSeqNr,
    ) -> Result<bool, Self::Error> {
        if node == self.root_id {
            return Err(PetVicinityGraphError::Root);
        }
        self.valid_neighbor(&node, discovered_via)?;

        self.entries
            .entry(node)
            .and_modify(|entry| entry.update_observed_ssn(observed_ssn))
            .or_insert_with(|| Entry::new(observed_ssn));

        let new_edge = self.graph.add_edge(node, *discovered_via, ()).is_none();
        Ok(new_edge)
    }

    fn remove(&mut self, node: &NodeId) -> bool {
        let entry_removed = self.entries.remove(node).is_some();
        let graph_removed = self.graph.remove_node(*node);

        assert!(
            !entry_removed || graph_removed, // entry_removed => graph_removed
            "vicinity node entry not in the vicinity graph"
        );

        entry_removed || graph_removed
    }

    fn entry(&self, node: &NodeId) -> Option<&Entry> {
        self.entries.get(node)
    }

    fn entry_mut(&mut self, node: &NodeId) -> Option<&mut Entry> {
        self.entries.get_mut(node)
    }

    fn vicinity(&self, node: &NodeId) -> impl Iterator<Item = NodeId> {
        self.graph.neighbors(*node)
    }

    fn root_distance(&self, node: &NodeId) -> Option<usize> {
        if self.graph.contains_edge(self.root_id, *node) {
            Some(1)
        } else {
            dijkstra(&self.graph, self.root_id, Some(*node), |_| 1)
                .get(node)
                .copied()
        }
    }

    fn remove_edge(&mut self, node_a: &NodeId, node_b: &NodeId) -> bool {
        self.graph.remove_edge(*node_a, *node_b).is_some()
    }

    fn retain_vicinity(&mut self) -> impl Iterator<Item = NodeId> {
        let distances = dijkstra(&self.graph, self.root_id, None, |_| 1);
        let nodes: Vec<_> = self.nodes().collect(); // required to (rightfully) satisfy borrow checker
        nodes.into_iter().filter(move |n| {
            let outside = distances
                .get(n)
                .is_none_or(|rdistance| *rdistance >= VICINITY_RADIUS);
            if outside {
                assert!(
                    self.remove(n),
                    "Removing a vicinity node should mark it changed"
                )
            };
            outside
        })
    }

    fn nodes(&self) -> impl Iterator<Item = NodeId> {
        self.graph.nodes().filter(|node| node != &self.root_id)
    }

    fn vicinity_paths(&self) -> impl Iterator<Item = Path> {
        self.nodes()
            .flat_map(|node| {
                all_simple_paths::<_, _, RandomState>(
                    &self.graph,
                    self.root_id,
                    node,
                    0,
                    MAX_INTERMEDIATE_NODES,
                )
            })
            .filter_map(Result::ok)
    }

    fn vicinity_paths_to(&self, destination: NodeId) -> impl Iterator<Item = Path> {
        // intermediate nodes: start and end node excluded

        all_simple_paths::<_, _, RandomState>(
            &self.graph,
            self.root_id,
            destination,
            0,
            MAX_INTERMEDIATE_NODES,
        )
        .filter_map(Result::ok)
    }

    fn vicinity_path_to(&self, destination: NodeId) -> Option<Path> {
        let (_, path) = astar(
            &self.graph,
            self.root_id,
            |n| n == destination,
            |_| 1,
            |_| 0,
        )?;
        if path.len() > VICINITY_RADIUS {
            return None;
        }

        path.try_into().ok()
    }
}

#[cfg(test)]
mod test {
    use std::collections::HashSet;
    use std::time::Instant;

    use super::*;

    #[test]
    fn empty_graph() {
        let root_id = NodeId::with_msb(1);
        let graph = PetVicinityGraph::new(root_id);

        assert_eq!(graph.entry(&root_id), None);

        assert_eq!(
            graph.nodes().collect::<Vec<_>>(),
            vec![],
            "no nodes in empty graph"
        );

        assert_eq!(
            graph.vicinity_paths().collect::<Vec<_>>(),
            vec![],
            "no paths in empty graph"
        );
    }

    #[test]
    fn no_insertion_of_root_id() {
        let root_id = NodeId::with_msb(1);
        let mut graph = PetVicinityGraph::new(root_id);

        assert!(
            graph
                .insert(root_id, &root_id, SafeStateSeqNr::try_from(1).unwrap())
                .is_err(),
            "root can't be inserted in the vicinity"
        );
    }

    #[test]
    fn insert_entry() {
        let root_id = NodeId::with_msb(1);
        let insert_node = NodeId::with_msb(2);
        let insert_ssn = SafeStateSeqNr::try_from(1).unwrap();

        let mut graph = PetVicinityGraph::new(root_id);

        let insertion_res = graph.insert(insert_node, &root_id, insert_ssn);
        assert!(insertion_res.is_ok(), "insert node via root");
        assert!(insertion_res.unwrap(), "insert new edge");
        assert!(
            graph.graph.contains_edge(root_id, insert_node),
            "contains edge to neighbor after insert"
        );
        assert_eq!(
            graph.root_distance(&insert_node),
            Some(1),
            "inserted uln has distance 1 to root"
        );

        // check entry creation
        let entry = graph.entry(&insert_node).expect("insertion creates entry");
        assert_eq!(entry.observed_ssn(), &insert_ssn);
        assert_eq!(entry.vicinity_ssn(), None);
        assert_eq!(entry.last_seen(), None);
    }

    #[test]
    fn update_via_insert() {
        let root_id = NodeId::with_msb(1);
        let insert_node = NodeId::with_msb(2);
        let initial_ssn = SafeStateSeqNr::try_from(1).unwrap();
        let updated_ssn = SafeStateSeqNr::try_from(2).unwrap();
        let now = Instant::now();
        let vicinity_ssn = initial_ssn;
        let inital_entry = {
            let mut initial_entry = Entry::new(initial_ssn);
            initial_entry.update_last_seen(now);
            initial_entry.update_vicinity_ssn(vicinity_ssn);
            initial_entry
        };

        // prep graph with existing link to insert_node
        let mut graph = PetVicinityGraph::new(root_id);
        graph.graph.add_edge(root_id, insert_node, ());
        graph.entries.insert(insert_node, inital_entry);

        // same ssn should leave other data "unharmed"
        {
            let insertion_res = graph.insert(insert_node, &root_id, initial_ssn);
            assert!(insertion_res.is_ok(), "insert same data again");
            assert!(!insertion_res.unwrap(), "don't insert new edge");

            // check entry
            let entry = graph.entry(&insert_node).expect("insertion creates entry");
            assert_eq!(entry.observed_ssn(), &initial_ssn);
            assert_eq!(entry.vicinity_ssn(), Some(&vicinity_ssn));
            assert_eq!(entry.last_seen(), Some(now));
        }

        {
            let insertion_res = graph.insert(insert_node, &root_id, updated_ssn);
            assert!(insertion_res.is_ok(), "insert updated ssn");
            assert!(!insertion_res.unwrap(), "don't insert new edge");

            // check entry
            let entry = graph.entry(&insert_node).expect("insertion creates entry");
            assert_eq!(entry.observed_ssn(), &updated_ssn);
            assert_eq!(entry.vicinity_ssn(), Some(&vicinity_ssn));
            assert_eq!(entry.last_seen(), Some(now));
        }
    }

    #[test]
    fn calculates_all_paths_in_2_hop_vicinity() {
        /*
        Topology:
            /- 2 -\
           1 - 3 - 4 - 5
           |   |       |
           \-- 6 -- 7 -/

        Vicinity of N=1:
            /- 2 -\
           1 - 3 - 4
           |   |
           \-- 6 -- 7
        */

        // same ssn for all nodes for now
        let ssn = SafeStateSeqNr::try_from(1).unwrap();
        let n = [
            NodeId::with_msb(1),
            NodeId::with_msb(1),
            NodeId::with_msb(2),
            NodeId::with_msb(3),
            NodeId::with_msb(4),
            NodeId::with_msb(5),
            NodeId::with_msb(6),
            NodeId::with_msb(7),
        ];
        let root = n[0];

        // build graph
        let graph = {
            let mut graph = PetVicinityGraph::new(root);
            // 2
            assert!(graph.insert(n[2], &n[1], ssn).is_ok());
            assert!(graph.insert(n[4], &n[2], ssn).is_ok());

            // 3
            assert!(graph.insert(n[3], &n[1], ssn).is_ok());
            assert!(graph.insert(n[4], &n[3], ssn).is_ok());
            assert!(graph.insert(n[6], &n[3], ssn).is_ok());

            // 6
            assert!(graph.insert(n[6], &n[1], ssn).is_ok());
            assert!(graph.insert(n[3], &n[6], ssn).is_ok());
            assert!(graph.insert(n[7], &n[6], ssn).is_ok());

            // 4
            assert!(graph.insert(n[4], &n[3], ssn).is_ok());
            assert!(graph.insert(n[2], &n[4], ssn).is_ok());
            assert!(graph.insert(n[5], &n[4], ssn).is_ok());

            // 7
            assert!(graph.insert(n[7], &n[6], ssn).is_ok());
            assert!(graph.insert(n[5], &n[7], ssn).is_ok());

            graph
        };

        let mut paths = graph.vicinity_paths().collect::<HashSet<_>>();
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
    fn retain_vicinity() {
        /*
        Topology:
            /- 2 -\
           1 - 3 - 4 - 5
           |   |       |
           \-- 6 -- 7 -/

        Vicinity of N=1:
            /- 2 -\
           1 - 3 - 4
           |   |
           \-- 6 -- 7
        */

        // same ssn for all nodes for now
        let ssn = SafeStateSeqNr::try_from(1).unwrap();
        let n = [
            NodeId::with_msb(1),
            NodeId::with_msb(1),
            NodeId::with_msb(2),
            NodeId::with_msb(3),
            NodeId::with_msb(4),
            NodeId::with_msb(5),
            NodeId::with_msb(6),
            NodeId::with_msb(7),
        ];
        let root = n[0];

        // build graph
        let mut graph = PetVicinityGraph::new(root);
        // 2
        assert!(graph.insert(n[2], &n[1], ssn).is_ok());
        assert!(graph.insert(n[4], &n[2], ssn).is_ok());

        // 3
        assert!(graph.insert(n[3], &n[1], ssn).is_ok());
        assert!(graph.insert(n[4], &n[3], ssn).is_ok());
        assert!(graph.insert(n[6], &n[3], ssn).is_ok());

        // 6
        assert!(graph.insert(n[6], &n[1], ssn).is_ok());
        assert!(graph.insert(n[3], &n[6], ssn).is_ok());
        assert!(graph.insert(n[7], &n[6], ssn).is_ok());

        // 4
        assert!(graph.insert(n[4], &n[3], ssn).is_ok());
        assert!(graph.insert(n[2], &n[4], ssn).is_ok());
        assert!(graph.insert(n[5], &n[4], ssn).is_ok());

        // 7
        assert!(graph.insert(n[7], &n[6], ssn).is_ok());
        assert!(graph.insert(n[5], &n[7], ssn).is_ok());

        let mut removed = false;
        for removed_node in graph.retain_vicinity() {
            if removed_node == n[5] {
                removed = true;
            } else {
                panic!("retain removed unexpected node: {removed_node}");
            }
        }
        if !removed {
            panic!("not removed node outside vicinity: {}", n[5]);
        }

        assert!(
            !graph.entries.contains_key(&n[5]),
            "remove node entries on retain_vicinity"
        );
    }
}
