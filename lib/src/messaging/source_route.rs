use std::collections::VecDeque;

use crate::domain::{NodeId, Path};

/// Source Route of a package all the way back to its origin.
///
/// This is a different Type than [Path] as the requirements are different.
/// While Path only provides access based on the allowed functions to a Contact
/// the source route is altered in a Message context.
///
/// Invariant:
/// - Current_hop has to be the current nodes NodeId.
/// - SourceRoutes are not allowed to be empty and always start with the source of a [ProtocolMessage].
/// - progress is in range [1, len - 1].
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

    /// Creates a new source path with 0 progress and reversed of the given route.
    pub fn from_reversed<I: Into<SourceRoute>>(route: I) -> Self {
        let mut converted = route.into();
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

    /// Advances the source routes progress by one returning the previous position.
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
        self.progress == self.ids.len() - 1
    }

    /// Returns the first element of the [SourceRoute].
    ///
    /// This is in general the source node of the [ProtocolMessage].
    pub fn source(&self) -> &NodeId {
        self.ids.front().expect("constructed empty SourceRoute")
    }

    /// Returns the last element of the [SourceRoute].
    ///
    /// This is in general the source node of the [ProtocolMessage].
    pub fn target(&self) -> &NodeId {
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
