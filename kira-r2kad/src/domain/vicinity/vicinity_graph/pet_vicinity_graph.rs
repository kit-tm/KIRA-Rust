//! A [VicinityGraph] implementation supported by [petgraph].

use std::{collections::HashMap, hash::RandomState, iter, time::Instant};

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
    #[display("No neighbor of {node} inside the VicinityGraph: {neighbors:#?}")]
    NeighborNotFound {
        #[error(ignore)]
        node: NodeId,
        #[error(ignore)]
        neighbors: Vec<NodeId>,
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
    fn valid_neighbors(
        &self,
        node: &NodeId,
        neighbors: impl IntoIterator<Item = NodeId>,
    ) -> Result<Vec<NodeId>, PetVicinityGraphError> {
        let neighbors: Vec<_> = neighbors.into_iter().collect();
        // guard against unconnected nodes
        // and nodes that are not connected to contacted nodes
        if neighbors
            .iter()
            .any(|neighbor| neighbor == &self.root_id || self.entries.contains_key(neighbor))
        {
            Ok(neighbors)
        } else {
            Err(PetVicinityGraphError::NeighborNotFound {
                node: *node,
                neighbors,
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
    ) -> Result<(), Self::Error> {
        self.valid_neighbors(&node, iter::once(*discovered_via))?;

        self.entries.insert(node, Entry::new(observed_ssn));

        Ok(())
    }

    fn update_vicinity(
        &mut self,
        node: &NodeId,
        neighbors: impl IntoIterator<Item = NodeId>,
        vicinity_ssn: SafeStateSeqNr,
    ) -> Result<(), Self::Error> {
        let connected_neighbors = self.valid_neighbors(node, neighbors)?;

        self.graph.remove_node(*node);
        for neighbor in connected_neighbors {
            self.graph.add_edge(*node, neighbor, ());
        }

        self.entries
            .entry(*node)
            .and_modify(|entry| entry.update_vicinity_ssn(vicinity_ssn))
            .or_insert_with(|| Entry::new(vicinity_ssn));

        Ok(())
    }

    fn reset(&mut self, node: &NodeId) {
        let Some(entry) = self.entries.get_mut(node) else {
            return;
        };

        // expect at least the minimum ssn after a reset
        entry.update_observed_ssn(SafeStateSeqNr::MIN);
        entry.forget_vicinity();
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

    fn update_last_seen(&mut self, node: &NodeId, now: Instant) {
        let Some(entry) = self.entries.get_mut(node) else {
            return;
        };
        entry.update_last_seen(now);
    }

    fn update_observed_ssn(&mut self, node: &NodeId, observed_ssn: SafeStateSeqNr) {
        let Some(entry) = self.entries.get_mut(node) else {
            return;
        };
        entry.update_observed_ssn(observed_ssn);
    }

    fn last_seen(&self, node: &NodeId) -> Option<Instant> {
        self.entries.get(node).and_then(Entry::last_seen)
    }

    fn observed_ssn(&self, node: &NodeId) -> Option<&SafeStateSeqNr> {
        self.entries.get(node).map(Entry::observed_ssn)
    }

    fn vicinity(&self, node: &NodeId) -> impl Iterator<Item = NodeId> {
        self.graph.neighbors(*node)
    }

    fn vicinity_ssn(&self, node: &NodeId) -> Option<&SafeStateSeqNr> {
        self.entries.get(node).and_then(Entry::vicinity_ssn)
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

    fn retain_vicinity(&mut self) -> impl Iterator<Item = NodeId> {
        let distances = dijkstra(&self.graph, self.root_id, None, |_| 1);
        let nodes: Vec<_> = self.nodes().collect(); // required to (rightfully) satisfy borrow checker
        nodes.into_iter().filter(move |n| {
            let outside = distances
                .get(n)
                .is_none_or(|rdistance| *rdistance > VICINITY_RADIUS);
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
        self.graph
            .nodes()
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
