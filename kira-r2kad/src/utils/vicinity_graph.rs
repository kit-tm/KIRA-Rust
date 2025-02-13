use std::collections::{HashMap, HashSet};

use crate::domain::{NodeId, Path};

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Entry {
    valid: bool,
    neighbors: HashSet<NodeId>,
}

impl Entry {
    fn new(neighbors: HashSet<NodeId>) -> Self {
        Self {
            valid: true,
            neighbors,
        }
    }
}

/// Graph which generates all [Path]s from a given root [NodeId].
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct VicinityGraph {
    root_id: NodeId,
    // All underlay neighbors of a Node and the node itself
    pub neighbors: HashMap<NodeId, Entry>,
}

impl VicinityGraph {
    pub fn new(root_id: NodeId) -> Self {
        let mut neighbors = HashMap::default();
        neighbors.insert(root_id.clone(), Entry::new(HashSet::default()));
        Self { root_id, neighbors }
    }

    /// Inserts a node into the [VicinityGraph] with its underlay neighbors.
    ///
    /// Overwrites any existing entries.
    ///
    /// Returns the previously present neighbors for the node.
    pub fn insert(
        &mut self,
        node: NodeId,
        underlay_neighbors: HashSet<NodeId>,
    ) -> Option<HashSet<NodeId>> {
        // Ensure bidirectional links
        for neighbor in &underlay_neighbors {
            if let Some(neighbors_of_neighbor) = self.neighbors.get_mut(neighbor) {
                neighbors_of_neighbor.neighbors.insert(node.clone());
            }
        }
        self.neighbors
            .insert(node, Entry::new(underlay_neighbors))
            .map(|entry| entry.neighbors)
    }

    /// Adds the given underlay neighbors of the node to the [VicinityGraph].
    ///
    /// If no entry is present it will be created and if an entry is already present they will
    /// be extended with the given neighbors instead of replacing.
    /// If replacement is required the method [insert](VicinityGraph::insert) should be used.
    pub fn add(&mut self, node: NodeId, underlay_neighbors: HashSet<NodeId>) {
        // Ensure bidirectional links
        for neighbor in &underlay_neighbors {
            if let Some(neighbors_of_neighbor) = self.neighbors.get_mut(neighbor) {
                neighbors_of_neighbor.neighbors.insert(node);
            }
        }

        if let Some(neighbors) = self.neighbors.get_mut(&node) {
            neighbors.neighbors.extend(underlay_neighbors);
        } else {
            self.neighbors.insert(node, Entry::new(underlay_neighbors));
        }
    }

    /// Removes a node from the [VicinityGraph].
    pub fn remove(&mut self, node: &NodeId) -> Option<HashSet<NodeId>> {
        if node == &self.root_id {
            return None;
        }
        let neighbors = self.neighbors.remove(node).map(|entry| entry.neighbors);
        if let Some(neighbors) = &neighbors {
            for neighbor in neighbors {
                if let Some(neighbors_of_neighbor) = self.neighbors.get_mut(neighbor) {
                    neighbors_of_neighbor.neighbors.remove(node);
                }
            }
        }
        neighbors
    }

    /// Invalidates the entry for the given [NodeId].
    ///
    /// Returns if an entry was found and invalidated or no entry was found.
    pub fn invalidate(&mut self, node: &NodeId) -> bool {
        if let Some(entry) = self.neighbors.get_mut(node) {
            entry.valid = false;
            return true;
        }

        false
    }

    /// Validates the entry for the given [NodeId].
    ///
    /// Returns if an entry was found and validated or no entry was found.
    pub fn validate(&mut self, node: &NodeId) -> bool {
        if let Some(entry) = self.neighbors.get_mut(node) {
            entry.valid = true;
            return true;
        }

        false
    }

    /// Checks if and entry for the given [NodeId] exists.
    ///
    /// Returns if an entry was found with the given [NodeId].
    pub fn contains(&self, node: &NodeId) -> bool {
        self.neighbors.contains_key(node)
            || self
                .neighbors
                .values()
                .any(|entry| entry.neighbors.contains(node))
    }
}

impl IntoIterator for &VicinityGraph {
    type Item = Path;
    type IntoIter = std::vec::IntoIter<Path>;

    fn into_iter(self) -> Self::IntoIter {
        let mut paths = Vec::with_capacity(self.neighbors.len() * self.neighbors.len());
        for (neigh1_id, neigh1_entry) in self.neighbors.iter() {
            if !neigh1_entry.valid || neigh1_id == &self.root_id {
                continue;
            }

            paths.push(Path::from([self.root_id, *neigh1_id]));

            for neigh2_id in neigh1_entry.neighbors.iter() {
                if neigh2_id == &self.root_id {
                    continue;
                }
                paths.push(Path::from([self.root_id, *neigh1_id, *neigh2_id]));
            }
        }
        paths.into_iter()
    }
}

impl Extend<(NodeId, HashSet<NodeId>)> for VicinityGraph {
    fn extend<T: IntoIterator<Item = (NodeId, HashSet<NodeId>)>>(&mut self, iter: T) {
        for (id, pns) in iter {
            self.insert(id, pns);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use crate::domain::{NodeId, Path};
    use crate::utils::vicinity_graph::VicinityGraph;

    #[test]
    fn calculates_all_paths_in_3_hop_vicinity() {
        /*
        Topology:

             /- 2 -\
           1 -- 3 -- 4 -- 5
                |         |
             `- 6 -- 7 -´

        */

        let root_id = NodeId::with_msb(1);

        let mut graph = VicinityGraph::new(root_id);
        graph.insert(
            NodeId::with_msb(1),
            HashSet::from_iter([
                NodeId::with_msb(2),
                NodeId::with_msb(3),
                NodeId::with_msb(6),
            ]),
        );
        graph.insert(
            NodeId::with_msb(2),
            HashSet::from_iter([NodeId::with_msb(1), NodeId::with_msb(4)]),
        );
        graph.insert(
            NodeId::with_msb(3),
            HashSet::from_iter([
                NodeId::with_msb(1),
                NodeId::with_msb(4),
                NodeId::with_msb(6),
            ]),
        );
        graph.insert(
            NodeId::with_msb(4),
            HashSet::from_iter([
                NodeId::with_msb(2),
                NodeId::with_msb(3),
                NodeId::with_msb(5),
            ]),
        );
        graph.insert(
            NodeId::with_msb(5),
            HashSet::from_iter([NodeId::with_msb(4), NodeId::with_msb(7)]),
        );
        graph.insert(
            NodeId::with_msb(6),
            HashSet::from_iter([
                NodeId::with_msb(1),
                NodeId::with_msb(3),
                NodeId::with_msb(7),
            ]),
        );
        graph.insert(
            NodeId::with_msb(7),
            HashSet::from_iter([NodeId::with_msb(6), NodeId::with_msb(5)]),
        );

        let mut paths = graph.into_iter().collect::<HashSet<_>>();
        let expected_paths = HashSet::from([
            // All paths to 2
            Path::from([NodeId::with_msb(1), NodeId::with_msb(2)]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(3),
                NodeId::with_msb(4),
                NodeId::with_msb(2),
            ]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(6),
                NodeId::with_msb(3),
                NodeId::with_msb(4),
                NodeId::with_msb(2),
            ]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(6),
                NodeId::with_msb(7),
                NodeId::with_msb(5),
                NodeId::with_msb(4),
                NodeId::with_msb(2),
            ]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(3),
                NodeId::with_msb(6),
                NodeId::with_msb(7),
                NodeId::with_msb(5),
                NodeId::with_msb(4),
                NodeId::with_msb(2),
            ]),
            // All paths to 3
            Path::from([NodeId::with_msb(1), NodeId::with_msb(3)]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(2),
                NodeId::with_msb(4),
                NodeId::with_msb(3),
            ]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(2),
                NodeId::with_msb(4),
                NodeId::with_msb(5),
                NodeId::with_msb(7),
                NodeId::with_msb(6),
                NodeId::with_msb(3),
            ]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(6),
                NodeId::with_msb(3),
            ]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(6),
                NodeId::with_msb(7),
                NodeId::with_msb(5),
                NodeId::with_msb(4),
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
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(3),
                NodeId::with_msb(6),
                NodeId::with_msb(7),
                NodeId::with_msb(5),
                NodeId::with_msb(4),
            ]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(6),
                NodeId::with_msb(3),
                NodeId::with_msb(4),
            ]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(6),
                NodeId::with_msb(7),
                NodeId::with_msb(5),
                NodeId::with_msb(4),
            ]),
            // All paths to 5
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(2),
                NodeId::with_msb(4),
                NodeId::with_msb(5),
            ]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(2),
                NodeId::with_msb(4),
                NodeId::with_msb(3),
                NodeId::with_msb(6),
                NodeId::with_msb(7),
                NodeId::with_msb(5),
            ]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(3),
                NodeId::with_msb(4),
                NodeId::with_msb(5),
            ]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(3),
                NodeId::with_msb(6),
                NodeId::with_msb(7),
                NodeId::with_msb(5),
            ]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(6),
                NodeId::with_msb(7),
                NodeId::with_msb(5),
            ]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(6),
                NodeId::with_msb(3),
                NodeId::with_msb(4),
                NodeId::with_msb(5),
            ]),
            // All paths to 6
            Path::from([NodeId::with_msb(1), NodeId::with_msb(6)]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(3),
                NodeId::with_msb(6),
            ]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(3),
                NodeId::with_msb(4),
                NodeId::with_msb(5),
                NodeId::with_msb(7),
                NodeId::with_msb(6),
            ]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(2),
                NodeId::with_msb(4),
                NodeId::with_msb(3),
                NodeId::with_msb(6),
            ]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(2),
                NodeId::with_msb(4),
                NodeId::with_msb(5),
                NodeId::with_msb(7),
                NodeId::with_msb(6),
            ]),
            // All paths to 7
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(6),
                NodeId::with_msb(7),
            ]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(6),
                NodeId::with_msb(3),
                NodeId::with_msb(4),
                NodeId::with_msb(5),
                NodeId::with_msb(7),
            ]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(3),
                NodeId::with_msb(6),
                NodeId::with_msb(7),
            ]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(3),
                NodeId::with_msb(4),
                NodeId::with_msb(5),
                NodeId::with_msb(7),
            ]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(2),
                NodeId::with_msb(4),
                NodeId::with_msb(3),
                NodeId::with_msb(6),
                NodeId::with_msb(7),
            ]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(2),
                NodeId::with_msb(4),
                NodeId::with_msb(5),
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
            "Not generated paths: {:#?}; Additionally generated paths: {:#?}",
            not_generated,
            paths
        );
    }

    #[test]
    fn calculates_only_bidirectionally_approved_paths() {
        /*
        Topology:

             /- 2 -\
           1 -- 3 -- 4

        */

        let root_id = NodeId::with_msb(1);

        let mut graph = VicinityGraph::new(root_id);
        graph.insert(
            NodeId::with_msb(1),
            HashSet::from_iter([NodeId::with_msb(2), NodeId::with_msb(3)]),
        );
        graph.insert(
            NodeId::with_msb(2),
            HashSet::from_iter([NodeId::with_msb(1), NodeId::with_msb(4)]),
        );
        graph.insert(
            NodeId::with_msb(4),
            HashSet::from_iter([NodeId::with_msb(2), NodeId::with_msb(3)]),
        );

        let mut paths = graph.into_iter().collect::<HashSet<_>>();
        let expected_paths = HashSet::from([
            Path::from([NodeId::with_msb(1), NodeId::with_msb(2)]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(2),
                NodeId::with_msb(4),
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
            "Not generated paths: {:#?}; Additionally generated paths: {:#?}",
            not_generated,
            paths
        );
    }

    #[test]
    fn calculates_no_entries_for_invalidated_entries() {
        /*
        Topology:

             /- 2 -\
           1 -- 3 -- 4

        */

        let root_id = NodeId::with_msb(1);

        let mut graph = VicinityGraph::new(root_id);
        graph.insert(
            NodeId::with_msb(1),
            HashSet::from_iter([NodeId::with_msb(2), NodeId::with_msb(3)]),
        );
        graph.insert(
            NodeId::with_msb(2),
            HashSet::from_iter([NodeId::with_msb(1), NodeId::with_msb(4)]),
        );
        graph.insert(
            NodeId::with_msb(3),
            HashSet::from_iter([NodeId::with_msb(1), NodeId::with_msb(4)]),
        );
        graph.insert(
            NodeId::with_msb(4),
            HashSet::from_iter([NodeId::with_msb(2), NodeId::with_msb(3)]),
        );
        assert!(
            graph.invalidate(&NodeId::with_msb(3)),
            "Failed to invalidate node 3"
        );

        let mut paths = graph.into_iter().collect::<HashSet<_>>();
        let expected_paths = HashSet::from([
            Path::from([NodeId::with_msb(1), NodeId::with_msb(2)]),
            Path::from([
                NodeId::with_msb(1),
                NodeId::with_msb(2),
                NodeId::with_msb(4),
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
            "Not generated paths: {:#?}; Additionally generated paths: {:#?}",
            not_generated,
            paths
        );
    }
}
