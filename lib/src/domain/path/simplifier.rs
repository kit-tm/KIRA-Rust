use crate::domain::{PNTable, Path, RoutingTable};

/// An algorithm to simplify/shorten a [Path] with the information given in a
/// [RoutingTable] and [PNTable].
pub trait PathSimplifier {
    fn simplify<RT, const BUCKET_SIZE: usize>(
        &mut self,
        routing_table: &RT,
        pn_table: &PNTable,
        path: &mut Path,
    ) where
        RT: RoutingTable<BUCKET_SIZE>;
}
