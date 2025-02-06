use crate::domain::{Path, RoutingTable, UNTable};

/// An algorithm to simplify/shorten a [Path] with the information given in a
/// [RoutingTable] and [PNTable].
pub trait PathSimplifier {
    fn simplify<RT, PN, const BUCKET_SIZE: usize>(
        &mut self,
        routing_table: &RT,
        un_table: &PN,
        path: &mut Path,
    ) where
        for<'a> RT: RoutingTable<'a, BUCKET_SIZE>,
        PN: UNTable;
}
