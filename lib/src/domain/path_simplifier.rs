use std::ops::{Deref, DerefMut};

use super::{Contact, NeighborTable, NodeId, Path, Port, RoutingTable};

pub struct PathSimplifier<'a>(&'a mut Path);

impl<'a> From<&'a mut Path> for PathSimplifier<'a> {
    fn from(path: &'a mut Path) -> Self {
        Self(path)
    }
}

impl Deref for PathSimplifier<'_> {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl DerefMut for PathSimplifier<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl PathSimplifier<'_> {
    /// Simplifies the [Path] by replacing parts of it with known
    /// shorter [Path]s.
    pub fn simplify<RT, NT, const BUCKET_SIZE: usize>(
        &mut self,
        routing_table: &RT,
        neighbor_table: &NT,
    ) where
        RT: RoutingTable<BUCKET_SIZE>,
        for<'a> &'a RT: IntoIterator<Item = &'a Contact>,
        NT: NeighborTable,
        for<'b> &'b NT: IntoIterator<Item = (&'b NodeId, &'b Port)>,
    {
        // Already a neighbor, can't be shortened
        if self.0.len() <= 1 {
            return;
        }

        // Only paths TO a Node are known and we want to replace bigger paths first
        // Therefore we iterate from the back
        //
        // First replace all neighbors as these have the shortest path
        //
        // Not checking index 0, as neighbor paths can't be simplified
        for dest_index in (1..self.len()).rev() {
            let dest_id = self[dest_index].clone();

            // Replace if target is a neighbor
            if neighbor_table.contains(&dest_id) {
                self.replace_interval(0, dest_index, [dest_id]);
                // Breaking, as the remaining path to check is replaced by the neighbors path
                break;
            }
        }

        // Now we replace all non-neighbor paths
        for dest_index in (1..self.len()).rev() {
            let part_len = dest_index + 1;
            let dest_id = self[dest_index].clone();

            // Replace if a shorter path to destination is known in RT
            if let Some(known_contact) = routing_table.contact(&dest_id) {
                let whole_path = known_contact.whole_path();
                if whole_path.len() < part_len {
                    self.replace_interval(0, dest_index, whole_path);
                    break;
                }
            }
        }

        // This doesn't need to be done recursively, as all entries in the Routing table
        // are assumed to be shortest paths to their contacts.
        // If a new Path is simplified which contains shorter Paths to a Node they must
        // be updated after this
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::domain::{
        Age, Contact, FlatRoutingTable, NeighborTable, NodeId, Path, Port, RoutingTable, StateSeqNr,
    };

    use super::PathSimplifier;

    #[test]
    fn simplify_neighbor_part() -> Result<(), Box<dyn std::error::Error>> {
        let mut neighbor_table = HashMap::new();
        neighbor_table.add(NodeId::zero(), Port::new(String::from("0")));

        let routing_table = FlatRoutingTable::<20, 1>::new(NodeId::zero())?;

        let path = Path::from([
            NodeId::with_lsb(0b00001111),
            NodeId::with_lsb(0b11110000),
            NodeId::with_lsb(0),
            NodeId::with_lsb(0b10101010),
        ]);

        let mut cloned = path.clone();
        let mut simplifier = PathSimplifier::from(&mut cloned);

        simplifier.simplify(&routing_table, &neighbor_table);

        assert_ne!(*simplifier, path);
        assert_eq!(
            *simplifier,
            Path::from([NodeId::with_lsb(0), NodeId::with_lsb(0b10101010),])
        );

        Ok(())
    }

    #[test]
    fn simplify_neighbor_end() -> Result<(), Box<dyn std::error::Error>> {
        let mut neighbor_table = HashMap::new();
        neighbor_table.add(NodeId::zero(), Port::new(String::from("0")));

        let routing_table = FlatRoutingTable::<20, 1>::new(NodeId::zero())?;

        let path = Path::from([
            NodeId::with_lsb(0b00001111),
            NodeId::with_lsb(0b11110000),
            NodeId::with_lsb(0),
        ]);

        let mut cloned = path.clone();
        let mut simplifier = PathSimplifier::from(&mut cloned);

        simplifier.simplify(&routing_table, &neighbor_table);

        assert_ne!(*simplifier, path);
        assert_eq!(*simplifier, Path::from([NodeId::with_lsb(0),]));

        Ok(())
    }

    #[test]
    fn simplify_known_contact() -> Result<(), Box<dyn std::error::Error>> {
        let neighbor_table = HashMap::new();

        let mut routing_table = FlatRoutingTable::<20, 1>::new(NodeId::zero())?;
        routing_table.add(Contact::new(
            NodeId::zero(),
            Age::from(0),
            Path::from([NodeId::with_lsb(1), NodeId::with_lsb(2)]),
            StateSeqNr::from(0),
        ))?;

        let path = Path::from([
            NodeId::with_lsb(1),
            NodeId::with_lsb(2),
            NodeId::with_lsb(3),
            NodeId::zero(),
            NodeId::with_lsb(4),
        ]);

        let mut cloned = path.clone();
        let mut simplifier = PathSimplifier::from(&mut cloned);

        simplifier.simplify(&routing_table, &neighbor_table);

        assert_ne!(*simplifier, path);
        assert_eq!(
            *simplifier,
            Path::from([
                NodeId::with_lsb(1),
                NodeId::with_lsb(2),
                NodeId::zero(),
                NodeId::with_lsb(4),
            ])
        );

        Ok(())
    }

    #[test]
    fn simplify_multiple() -> Result<(), Box<dyn std::error::Error>> {
        let mut neighbor_table = HashMap::new();
        neighbor_table.add(NodeId::zero(), Port::new(String::from("0")));

        let mut routing_table = FlatRoutingTable::<20, 1>::new(NodeId::zero())?;
        routing_table.add(Contact::new(
            NodeId::one(),
            Age::from(0),
            Path::from([NodeId::with_lsb(1), NodeId::with_lsb(2)]),
            StateSeqNr::from(0),
        ))?;

        let path = Path::from([
            NodeId::with_lsb(2),
            NodeId::with_lsb(3),
            NodeId::with_lsb(4),
            NodeId::zero(),
            NodeId::with_lsb(1),
        ]);

        let mut cloned = path.clone();
        let mut simplifier = PathSimplifier::from(&mut cloned);

        simplifier.simplify(&routing_table, &neighbor_table);

        assert_ne!(*simplifier, path);
        assert_eq!(
            *simplifier,
            Path::from([NodeId::zero(), NodeId::with_lsb(1),])
        );

        Ok(())
    }
}
