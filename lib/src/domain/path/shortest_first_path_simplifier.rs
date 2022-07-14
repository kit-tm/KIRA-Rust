use crate::domain::simplifier::PathSimplifier;
use crate::domain::{PNTable, Path, RoutingTable};

pub struct ShortestFirstPathSimplifier;

impl PathSimplifier for ShortestFirstPathSimplifier {
    /// Simplifies the [Path] by replacing parts of it with known
    /// shorter [Path]s.
    fn simplify<RT, const BUCKET_SIZE: usize>(
        &mut self,
        routing_table: &RT,
        pn_table: &PNTable,
        path: &mut Path,
    ) where
        for<'a> RT: RoutingTable<'a, BUCKET_SIZE>,
    {
        // Already a physical neighbor, can't be shortened
        if path.size() <= 1 {
            return;
        }

        // Only paths TO a Node are known and we want to replace bigger paths first
        // Therefore we iterate from the back
        //
        // First replace all physical neighbors as these have the shortest path
        //
        // Not checking index 0, as physical neighbor paths can't be simplified
        for dest_index in (1..path.size()).rev() {
            let dest_id = path[dest_index].clone();

            // Replace if target is a physical neighbor
            if pn_table.contains(&dest_id) {
                path.replace_interval(0, dest_index, [dest_id]);
                // Breaking, as the remaining path to check is replaced by the physical neighbors path
                break;
            }
        }

        // Now we replace all non-physical-neighbor paths
        for dest_index in (1..path.size()).rev() {
            let part_len = dest_index + 1;
            let dest_id = path[dest_index].clone();

            // Replace if a shorter path to destination is known in RT
            if let Some(known_contact) = routing_table.contact(&dest_id) {
                let known_path = known_contact.path().clone();
                if known_path.size() < part_len {
                    path.replace_interval(0, dest_index, known_path);
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
    use crate::domain::{
        Age, Contact, FlatRoutingTable, NodeId, PNTable, Path, PathSimplifier, Port, RoutingTable,
        StateSeqNr,
    };

    use super::ShortestFirstPathSimplifier;

    #[test]
    fn simplify_pn_part() -> Result<(), Box<dyn std::error::Error>> {
        let mut pn_table = PNTable::new();
        pn_table.add(NodeId::zero(), Port::new(String::from("0")));

        let routing_table = FlatRoutingTable::<20, 1>::new(NodeId::zero())?;

        let path = Path::from([
            NodeId::with_lsb(0b00001111),
            NodeId::with_lsb(0b11110000),
            NodeId::with_lsb(0),
            NodeId::with_lsb(0b10101010),
        ]);

        let mut cloned = path.clone();

        ShortestFirstPathSimplifier.simplify(&routing_table, &pn_table, &mut cloned);

        assert_eq!(
            cloned,
            Path::from([NodeId::with_lsb(0), NodeId::with_lsb(0b10101010),])
        );

        Ok(())
    }

    #[test]
    fn simplify_pn_end() -> Result<(), Box<dyn std::error::Error>> {
        let mut pn_table = PNTable::new();
        pn_table.add(NodeId::zero(), Port::new(String::from("0")));

        let routing_table = FlatRoutingTable::<20, 1>::new(NodeId::zero())?;

        let path = Path::from([
            NodeId::with_lsb(0b00001111),
            NodeId::with_lsb(0b11110000),
            NodeId::with_lsb(0),
        ]);

        let mut cloned = path.clone();

        ShortestFirstPathSimplifier.simplify(&routing_table, &pn_table, &mut cloned);

        assert_eq!(cloned, Path::from([NodeId::with_lsb(0),]));

        Ok(())
    }

    #[test]
    fn simplify_known_contact() -> Result<(), Box<dyn std::error::Error>> {
        let pn_table = PNTable::new();

        let mut routing_table = FlatRoutingTable::<20, 1>::new(NodeId::zero())?;
        routing_table.add(Contact::new(
            Path::from([NodeId::with_lsb(1), NodeId::with_lsb(2), NodeId::zero()]),
            Age::from(0),
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

        ShortestFirstPathSimplifier.simplify(&routing_table, &pn_table, &mut cloned);

        assert_ne!(cloned, path);
        assert_eq!(
            cloned,
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
        let mut pn_table = PNTable::new();
        pn_table.add(NodeId::zero(), Port::new(String::from("0")));

        let mut routing_table = FlatRoutingTable::<20, 1>::new(NodeId::zero())?;
        routing_table.add(Contact::new(
            Path::from([NodeId::with_lsb(1), NodeId::with_lsb(2), NodeId::one()]),
            Age::from(0),
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

        ShortestFirstPathSimplifier.simplify(&routing_table, &pn_table, &mut cloned);

        assert_ne!(cloned, path);
        assert_eq!(cloned, Path::from([NodeId::zero(), NodeId::with_lsb(1),]));

        Ok(())
    }
}
