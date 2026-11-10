use bitflags::bitflags;

bitflags! {
    /// Labels describing what kind of node a [Contact](crate::domain::Contact) is.
    ///
    /// A contact may carry several labels at once. Labels come from different sources,
    /// which decide who may set or clear them:
    ///
    /// * [`DERIVED`](Self::DERIVED): computed from the contact itself, never stored.
    /// * [`LOCAL`](Self::LOCAL): decided by this node (e.g. via the API).
    /// * [`ADVERTISED`](Self::ADVERTISED): learned from other nodes (e.g. RTable objects).
    #[derive(Debug, Default, PartialEq, Eq, Clone, Copy, Hash)]
    pub struct ContactLabels: u8 {
        /// The contact is an underlay neighbor (its active path is a direct link).
        const Uln = 1;
        /// The contact should be kept in the routing table on request of this node.
        const Cached = 1 << 1;
        /// The node is an end-system that doesn't perform routing or forwarding.
        const EndSystem = 1 << 2;
        /// The node is a landmark.
        const Landmark = 1 << 3;
    }
}

impl ContactLabels {
    /// Labels advertised by other nodes.
    pub const ADVERTISED: Self = Self::EndSystem.union(Self::Landmark);
    /// Labels computed from the contact's state.
    pub const DERIVED: Self = Self::Uln;
    /// Labels set locally by this node.
    pub const LOCAL: Self = Self::Cached;
}

#[cfg(test)]
mod tests {
    use super::ContactLabels;

    #[test]
    fn sources_are_disjoint_and_complete() {
        let sources = [
            ContactLabels::DERIVED,
            ContactLabels::LOCAL,
            ContactLabels::ADVERTISED,
        ];
        for (i, a) in sources.iter().enumerate() {
            for b in &sources[i + 1..] {
                assert!(!a.intersects(*b), "{a:?} and {b:?} overlap");
            }
        }
        assert_eq!(
            sources
                .into_iter()
                .fold(ContactLabels::empty(), |acc, s| acc | s),
            ContactLabels::all(),
            "every label needs a source"
        );
    }
}
