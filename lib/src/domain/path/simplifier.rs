use crate::domain::{NeighborTable, Path, RoutingTable};

pub trait PathSimplifier {
    fn simplify<RT, const BUCKET_SIZE: usize>(
        &mut self,
        routing_table: &RT,
        neighbor_table: &NeighborTable,
        path: &mut Path,
    ) where
        RT: RoutingTable<BUCKET_SIZE>;
}
