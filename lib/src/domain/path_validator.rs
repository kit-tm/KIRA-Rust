use std::ops::Deref;

use super::{Contact, NeighborTable, NodeId, Path, Port, RoutingTable};

pub struct PathValidator(pub Path);

impl From<Path> for PathValidator {
    fn from(path: Path) -> Self {
        Self(path)
    }
}

impl Deref for PathValidator {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl PathValidator {
    /// Returns if the [Path] is valid to be inserted into the [RoutingTable].
    ///
    /// Valid means:
    ///
    /// - The first [NodeId] in [Path] is a neighbor
    /// -
    pub fn validate<RT, NT, const BUCKET_SIZE: usize>(
        &self,
        routing_table: RT,
        neighbor_table: NT,
    ) -> bool
    where
        RT: RoutingTable<BUCKET_SIZE>,
        for<'a> &'a RT: IntoIterator<Item = &'a Contact>,
        NT: NeighborTable,
        for<'b> &'b NT: IntoIterator<Item = (&'b NodeId, &'b Port)>,
    {
        // First node id is a neighbor
        let first = self
            .0
            .first()
            .and_then(|id| routing_table.contact(id))
            .map(|contact| contact.id());
        if first.is_none() || !neighbor_table.contains(first.unwrap()) {
            return false;
        }

        true
    }
}
