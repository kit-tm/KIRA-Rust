use crate::domain::simplifier::PathSimplifier;
use crate::domain::{ContactState, Path, RoutingTable, ULNTable};

#[derive(Debug)]
pub struct ShortestFirstPathSimplifier;

impl PathSimplifier for ShortestFirstPathSimplifier {
    /// Simplifies the [Path] by replacing parts of it with known
    /// shorter [Path]s.
    fn simplify<RT, UN, const BUCKET_SIZE: usize>(
        &mut self,
        routing_table: &RT,
        uln_table: &UN,
        path: &mut Path,
    ) where
        for<'a> RT: RoutingTable<'a, BUCKET_SIZE>,
        UN: ULNTable,
    {
        // Already a underlay neighbor, can't be shortened
        if path.size() <= 1 {
            return;
        }

        // Only paths TO a Node are known and we want to replace bigger paths first
        // Therefore we iterate from the back
        //
        // First replace all underlay neighbors as these have the shortest path
        //
        // Not checking index 0, as underlay neighbor paths can't be simplified
        for dest_index in (1..path.size()).rev() {
            let dest_id = path[dest_index];

            // Replace if target is a underlay neighbor
            if uln_table.contains(&dest_id) {
                path.replace_interval(0, dest_index, [dest_id]);
                // Breaking, as the remaining path to check is replaced by the underlay neighbors path
                break;
            }
        }

        // Now we replace all non-underlay-neighbor paths
        for dest_index in (1..path.size()).rev() {
            let part_len = dest_index + 1;
            let dest_id = path[dest_index];

            // Replace if a shorter valid path to destination is known in RT
            if let Some(known_contact) = routing_table
                .contact(&dest_id)
                .filter(|contact| contact.state() == &ContactState::Valid)
            {
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
