use std::ops::Deref;

use super::{Path, RoutingTable, NeighborTable};

pub struct PathValidator<const ID_SIZE: usize>(pub Path<ID_SIZE>);

impl<const ID_SIZE: usize> From<Path<ID_SIZE>> for PathValidator<ID_SIZE> {
    fn from(path: Path<ID_SIZE>) -> Self {
        Self(path)
    }
}

impl<const ID_SIZE: usize> Deref for PathValidator<ID_SIZE> {
    type Target = Path<ID_SIZE>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<const ID_SIZE: usize> PathValidator<ID_SIZE> {
    /// Returns if the [Path] is valid to be inserted into the [RoutingTable].
    /// 
    /// Valid means:
    /// 
    /// - The first [NodeId] in [Path] is a neighbor
    /// - 
    pub fn validate<'a, RT, NT, const BUCKET_SIZE: usize>(
        &self,
        routing_table: RT,
        neighbor_table: NT,
    ) -> bool
        where
            RT: RoutingTable<'a, ID_SIZE, BUCKET_SIZE>,
            NT: NeighborTable<ID_SIZE>,
    {
        // First node id is a neighbor
        let first = self.0.first().and_then(|id| routing_table.contact(id)).map(|contact| contact.id());
        if first.is_none() || !neighbor_table.contains(first.unwrap()) {
            return false;
        }

        true
    }
}
