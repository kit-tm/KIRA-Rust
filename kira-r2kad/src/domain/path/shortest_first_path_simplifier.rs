use crate::domain::{
    ContactState,
    Path,
    RoutingTable,
    ULNTable,
    path::simplifier::PathSimplifier,
};

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
        // Only bidirectional underlay neighbors are used: the shortcut adds a direct link
        // the path didn't contain, and a tentative link is only known to work towards us.
        //
        // Not checking index 0, as underlay neighbor paths can't be simplified
        for dest_index in (1..path.size()).rev() {
            let dest_id = path[dest_index];

            // Replace if target is a bidirectional underlay neighbor
            if uln_table.is_bidirectional(&dest_id) {
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
                let known_path = known_contact
                    .path()
                    .expect("Valid contact should have active path")
                    .clone();
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
    use super::ShortestFirstPathSimplifier;
    use crate::domain::{
        Contact,
        ContactState,
        NodeId,
        Path,
        SafeStateSeqNr,
        ULNTable,
        UnderlayNeighborId,
        path::PathSimplifier,
        routing_table::{
            RoutingTable,
            SingleBucketRT,
        },
        underlay::{
            ConnectionId,
            InterfaceId,
        },
        underlay_neighbor_table::InMemoryULNTable,
    };

    const BUCKET_SIZE: usize = 8;

    fn ulnid() -> UnderlayNeighborId {
        UnderlayNeighborId {
            interface_id: InterfaceId::try_from(1).unwrap(),
            connection_id: ConnectionId::from(1),
        }
    }

    /// ULN table with tentative entries (only heard from, link not confirmed).
    fn tentative_uln_table_with(ids: &[NodeId]) -> InMemoryULNTable {
        let mut table = InMemoryULNTable::new();
        for id in ids {
            table.insert(*id, ulnid());
        }
        table
    }

    /// ULN table with bidirectional entries.
    fn uln_table_with(ids: &[NodeId]) -> InMemoryULNTable {
        let mut table = tentative_uln_table_with(ids);
        for id in ids {
            table.confirm(*id, ulnid());
        }
        table
    }

    fn routing_table_with_contact(path: &[NodeId]) -> SingleBucketRT<BUCKET_SIZE> {
        let mut table = SingleBucketRT::<BUCKET_SIZE>::new(NodeId::ZERO);
        table
            .add(Contact::new(
                Path::try_from(path).expect("test path must not be empty"),
                SafeStateSeqNr::try_from(1).unwrap(),
            ))
            .expect("bucket must have capacity");
        table
    }

    fn simplify(uln: &InMemoryULNTable, rt: &SingleBucketRT<BUCKET_SIZE>, path: &[NodeId]) -> Path {
        let mut path = Path::try_from(path).expect("test path must not be empty");
        ShortestFirstPathSimplifier.simplify(rt, uln, &mut path);
        path
    }

    #[test]
    fn single_hop_is_unchanged() {
        let uln = uln_table_with(&[NodeId::from(1)]);
        let rt = SingleBucketRT::<BUCKET_SIZE>::new(NodeId::ZERO);

        let path = simplify(&uln, &rt, &[NodeId::from(1)]);

        assert_eq!(path.size(), 1);
        assert_eq!(path.first(), &NodeId::from(1));
    }

    #[test]
    fn replaces_trailing_underlay_neighbor_hops() {
        // scanning from the back: NodeId(3) is an ULN, so the whole prefix is replaced
        let uln = uln_table_with(&[NodeId::from(1), NodeId::from(3)]);
        let rt = SingleBucketRT::<BUCKET_SIZE>::new(NodeId::ZERO);

        let path = simplify(
            &uln,
            &rt,
            &[NodeId::from(1), NodeId::from(2), NodeId::from(3)],
        );

        assert_eq!(path.size(), 1);
        assert_eq!(path.first(), &NodeId::from(3));
    }

    #[test]
    fn no_replacement_without_uln_or_known_path() {
        let uln = uln_table_with(&[NodeId::from(1)]);
        let rt = SingleBucketRT::<BUCKET_SIZE>::new(NodeId::ZERO);

        let path = simplify(
            &uln,
            &rt,
            &[NodeId::from(1), NodeId::from(2), NodeId::from(3)],
        );

        assert_eq!(path.size(), 3);
        assert_eq!(path.first(), &NodeId::from(1));
        assert_eq!(path.last(), &NodeId::from(3));
    }

    #[test]
    fn replaces_prefix_with_shorter_known_path() {
        let _clock = crate::tests::TestClock::new();
        // routing table knows a 2-hop path to NodeId(3)
        let uln = uln_table_with(&[NodeId::from(1)]);
        let rt = routing_table_with_contact(&[NodeId::from(1), NodeId::from(3)]);

        let path = simplify(
            &uln,
            &rt,
            &[NodeId::from(1), NodeId::from(2), NodeId::from(3)],
        );

        assert_eq!(path.size(), 2);
        assert_eq!(path.first(), &NodeId::from(1));
        assert_eq!(path.last(), &NodeId::from(3));
    }

    #[test]
    fn keeps_path_if_known_path_is_not_shorter() {
        let _clock = crate::tests::TestClock::new();
        // known path has the same length as the candidate path
        let uln = uln_table_with(&[NodeId::from(1)]);
        let rt = routing_table_with_contact(&[NodeId::from(1), NodeId::from(4), NodeId::from(3)]);

        let path = simplify(
            &uln,
            &rt,
            &[NodeId::from(1), NodeId::from(2), NodeId::from(3)],
        );

        assert_eq!(path.size(), 3);
        assert_eq!(path[1], NodeId::from(2), "path must be unchanged");
    }

    #[test]
    fn ignores_non_valid_contacts() {
        let _clock = crate::tests::TestClock::new();
        let mut rt = SingleBucketRT::<BUCKET_SIZE>::new(NodeId::ZERO);
        let mut contact = Contact::new(
            Path::try_from(&[NodeId::from(1), NodeId::from(3)][..]).unwrap(),
            SafeStateSeqNr::try_from(1).unwrap(),
        );
        contact.set_state(ContactState::Unknown);
        rt.add(contact).expect("bucket must have capacity");

        let uln = uln_table_with(&[NodeId::from(1)]);
        let path = simplify(
            &uln,
            &rt,
            &[NodeId::from(1), NodeId::from(2), NodeId::from(3)],
        );

        assert_eq!(path.size(), 3, "only valid contacts may shorten the path");
    }

    #[test]
    fn uln_replacement_wins_over_known_path() {
        let _clock = crate::tests::TestClock::new();
        // NodeId(3) is both an ULN and a contact in the routing table;
        // the ULN replacement happens first and terminates the simplification
        let uln = uln_table_with(&[NodeId::from(1), NodeId::from(3)]);
        let rt = routing_table_with_contact(&[NodeId::from(1), NodeId::from(3)]);

        let path = simplify(
            &uln,
            &rt,
            &[NodeId::from(1), NodeId::from(2), NodeId::from(3)],
        );

        assert_eq!(path.size(), 1);
        assert_eq!(path.first(), &NodeId::from(3));
    }

    #[test]
    fn does_not_shortcut_via_tentative_underlay_neighbor() {
        // <P, Q, B, S> with P bidirectional and B only tentative must not become <B, S>
        let (p, q, b, s) = (
            NodeId::from(1),
            NodeId::from(2),
            NodeId::from(3),
            NodeId::from(4),
        );
        let mut uln = uln_table_with(&[p]);
        uln.insert(b, ulnid());
        let rt = SingleBucketRT::<BUCKET_SIZE>::new(NodeId::ZERO);

        let path = simplify(&uln, &rt, &[p, q, b, s]);

        assert_eq!(path.into_iter().collect::<Vec<_>>(), [p, q, b, s]);
    }
}
