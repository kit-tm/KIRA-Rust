use crate::domain::{Path, RoutingTable, ULNTable};

/// An algorithm to simplify/shorten a [Path] with the information given in a
/// [RoutingTable] and [ULNTable].
pub trait PathSimplifier {
    fn simplify<RT, UN, const BUCKET_SIZE: usize>(
        &mut self,
        routing_table: &RT,
        un_table: &UN,
        path: &mut Path,
    ) where
        for<'a> RT: RoutingTable<'a, BUCKET_SIZE>,
        UN: ULNTable;
}
