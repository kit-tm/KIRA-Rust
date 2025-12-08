use std::collections::VecDeque;

use crate::domain::{NodeId, Path};

/// Source Route of a message all the way back to its origin.
///
/// This is a different Type than [Path] as the requirements are different.
/// While Path only provides access based on the allowed functions to a Contact
/// the source route is altered in a Message context.
///
/// Invariants:
/// - Current_hop has to be the current nodes NodeId.
/// - SourceRoutes are not allowed to be empty and always start with the source of a [ProtocolMessage](crate::messaging::messages::ProtocolMessage).
/// - progress is an index in the source route and therefore valid in range [0, len - 1]
#[derive(Debug, Eq, PartialEq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SourceRoute {
    ids: VecDeque<NodeId>,
    progress: usize,
}

impl SourceRoute {
    /// Creates a new [SourceRoute] starting from the given source appended with
    /// the given Path.
    pub fn new<I: Into<SourceRoute>>(source: NodeId, path: I) -> Self {
        let path = path.into();
        let mut ids = path.ids;
        ids.push_front(source);
        Self { ids, progress: 1 }
    }

    /// Creates a new source path from reverse of the given route.
    /// The given route is truncated at the current progress
    /// as this will be called by the responder
    pub fn from_reversed<I: Into<SourceRoute>>(route: I) -> Self {
        let mut converted = route.into();
        converted.ids.truncate(converted.progress + 1);
        converted.ids.make_contiguous().reverse();
        Self {
            ids: converted.ids,
            progress: 1,
        }
    }

    /// Insert a [NodeId] at the front of the [SourceRoute].
    ///
    /// The position in the [SourceRoute] will be moved back one element.
    /// To change that use [SourceRoute::advanced].
    ///
    /// Returns the [SourceRoute] itself to chain calls to mutating methods
    /// like [SourceRoute::advanced].
    pub fn push_front(&mut self, id: NodeId) -> &mut Self {
        self.ids.push_front(id);

        self
    }

    /// Returns the previous node in the [SourceRoute].
    pub fn prev_hop(&self) -> &NodeId {
        assert!(self.progress > 0);

        &self.ids[self.progress - 1]
    }

    /// Returns the next hop in the source route.
    pub fn next_hop(&self) -> Option<&NodeId> {
        if self.progress < self.ids.len() - 1 {
            return Some(&self.ids[self.progress + 1]);
        }

        None
    }

    /// Returns the current hop in the source route.
    pub fn current_hop(&self) -> &NodeId {
        if self.progress >= self.ids.len() {
            panic!("Source route advanced beyond the last element")
        }

        &self.ids[self.progress]
    }

    /// Advances the source route's progress by one
    pub fn advance(&mut self) {
        if self.progress < self.ids.len() - 1 {
            self.progress += 1;
        }
    }

    /// Returns the number of [NodeId]s in the source route.
    ///
    /// `size` not `len` as its idiomatic to provide an `is_empty` method for types providing a
    /// `len` method.
    pub fn size(&self) -> usize {
        self.ids.len()
    }

    /// Returns if the source route advanced beyond its length.
    ///
    /// This usually means the target was reached.
    pub fn is_finished(&self) -> bool {
        self.progress >= self.ids.len() - 1
    }

    /// Returns the first element of the [SourceRoute].
    ///
    /// This is in general the source node of the [ProtocolMessage](crate::messaging::messages::ProtocolMessage).
    pub fn source(&self) -> &NodeId {
        self.ids.front().expect("constructed empty SourceRoute")
    }

    /// Returns the last element of the [SourceRoute].
    ///
    /// This is in general the destination node of the [ProtocolMessage](crate::messaging::messages::ProtocolMessage).
    pub fn destination(&self) -> &NodeId {
        self.ids.back().expect("constructed empty SourceRoute")
    }

    /// Returns if the [SourceRoute] contains the given [NodeId].
    pub fn contains(&self, id: &NodeId) -> bool {
        self.ids.contains(id)
    }

    /// Advances the source routes progress by one returning the previous position.
    pub fn advanced(mut self) -> Self {
        self.advance();

        self
    }

    /// Returns the already traveled [SourceRoute].
    ///
    /// This doesn't include the current hop.
    pub fn traveled_path(&self) -> Path {
        assert!(self.progress > 0 && self.progress < self.size());

        let traveled_ids: Result<Path, _> = self.ids.iter().take(self.progress).cloned().collect();
        match traveled_ids {
            Ok(path) => path,
            Err(_) => panic!("Invalid invariant"),
        }
    }

    /// Returns the already traveled hop count of the [SourceRoute].
    ///
    /// This doesn't include the current hop.
    pub fn traveled_hop_count(&self) -> usize {
        assert!(self.progress > 0 && self.progress < self.size());
        self.progress
    }

    /// Returns the remaining [Path] including the current hop.
    pub fn remaining_path(&self) -> Path {
        assert!(self.progress > 0 && self.progress < self.size());
        let remaining_ids: Result<Path, _> = self.ids.iter().skip(self.progress).cloned().collect();
        match remaining_ids {
            Ok(path) => path,
            Err(_) => panic!("Invalid invariant"),
        }
    }
}

impl Extend<NodeId> for SourceRoute {
    fn extend<T: IntoIterator<Item = NodeId>>(&mut self, iter: T) {
        self.ids.extend(iter);
    }
}

impl From<Path> for SourceRoute {
    fn from(path: Path) -> Self {
        Self {
            ids: VecDeque::from_iter(path),
            progress: 1,
        }
    }
}

impl From<SourceRoute> for Path {
    fn from(route: SourceRoute) -> Self {
        route
            .ids
            .into_iter()
            .collect::<Result<Path, _>>()
            .expect("SourceRoute is not allowed to be empty")
    }
}

impl From<NodeId> for SourceRoute {
    fn from(id: NodeId) -> Self {
        Self {
            ids: VecDeque::from([id]),
            progress: 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SourceRoute;

    use super::{NodeId, Path};

    #[test]
    fn source_route_basics() {
        let first = NodeId::from(0x1);
        let second = NodeId::from(0x2);
        let third = NodeId::from(0x3);

        let mut src_route = SourceRoute::from(second);
        assert_eq!(src_route.size(), 1);
        assert!(src_route.is_finished());
        assert_eq!(*src_route.prev_hop(), second);
        src_route.push_front(first);
        assert_eq!(*src_route.current_hop(), second);
        assert_eq!(src_route.size(), 2);
        src_route.advance();
        assert_eq!(src_route.traveled_hop_count(), 1);
        assert_eq!(*src_route.current_hop(), second);
        assert!(src_route.contains(&second));
        let p = Path::from([first, second, third]);
        let src_route_2 = SourceRoute::from(p);
        assert_eq!(src_route_2.source(), &first);
        assert_eq!(src_route_2.destination(), &third);
        assert!(src_route_2.contains(&first));
        assert!(src_route_2.contains(&second));
        assert!(src_route_2.contains(&third));
    }

    #[test]
    fn source_route_reverse() {
        let first = NodeId::from(0x1);
        let second = NodeId::from(0x2);
        let third = NodeId::from(0x3);

        let p = Path::from([first, second, third]);
        let rev_p = Path::from([third, second, first]);
        let rev_p_trunc = Path::from([second, first]);
        let mut src_route = SourceRoute::from(p);
        let reverse_route = SourceRoute::from(rev_p_trunc);
        let reversed_route = SourceRoute::from_reversed(src_route.clone());
        assert_eq!(reversed_route, reverse_route);
        src_route.advance();
        assert!(src_route.is_finished());
        let reverse_route = SourceRoute::from(rev_p);
        let reversed_route = SourceRoute::from_reversed(src_route);
        assert_eq!(reversed_route, reverse_route);
    }
} // end tests
