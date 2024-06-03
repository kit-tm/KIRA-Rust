use crate::domain::{PNTable, Path, RoutingTable};

/// An algorithm to simplify/shorten a [Path] with the information given in a
/// [RoutingTable] and [PNTable].
pub trait PathSimplifier {
    fn simplify<RT, PN, const BUCKET_SIZE: usize>(
        &mut self,
        routing_table: &RT,
        pn_table: &PN,
        path: &mut Path,
    ) where
        for<'a> RT: RoutingTable<'a, BUCKET_SIZE>,
        PN: PNTable;
}
