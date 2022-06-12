use std::ops::Deref;

use super::{Path, RoutingTable, NeighborTable};


pub struct PathSimplifier<const ID_SIZE: usize>(Path<ID_SIZE>);

impl<const ID_SIZE: usize> From<Path<ID_SIZE>> for PathSimplifier<ID_SIZE> {
    fn from(path: Path<ID_SIZE>) -> Self {
        Self(path)
    }
}

impl<const ID_SIZE: usize> Deref for PathSimplifier<ID_SIZE> {
    type Target = Path<ID_SIZE>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<const ID_SIZE: usize> PathSimplifier<ID_SIZE> {
    pub fn simplify<'a, RT, NT, const BUCKET_SIZE: usize>(&mut self,
        routing_table: RT,
        neighbor_table: NT,
    )
        where
            RT: RoutingTable<'a, ID_SIZE, BUCKET_SIZE>,
            NT: NeighborTable<ID_SIZE>
    {
       if self.0.len() <= 1 {
           return;
       }
       todo!()
    }
}