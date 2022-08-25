use crate::domain::{NodeId, Path};
use std::error::Error;
use std::fmt::{Display, Formatter};

/// Source Route of a package all the way back to its origin.
///
/// This is a different Type than [Path] as the requirements are different.
/// While Path only provides access based on the allowed functions to a Contact
/// the source route is altered in a Message context.
///
/// Invariant:
/// - Current_hop has to be the current nodes NodeId unless the source route is empty.
/// - Progress is in range [0, len - 1].
#[derive(Debug, Eq, PartialEq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SourceRoute {
    ids: Vec<NodeId>,
    progress: usize,
}

impl SourceRoute {
    /// Creates a new source path with 0 progress and reversed of the given route.
    pub fn from_reversed(mut route: Self) -> Self {
        route.ids.reverse();
        Self {
            ids: route.ids,
            progress: 0,
        }
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
}

impl Extend<NodeId> for SourceRoute {
    fn extend<T: IntoIterator<Item = NodeId>>(&mut self, iter: T) {
        self.ids.extend(iter);
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct EmptyRouteError;

impl Display for EmptyRouteError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Route is empty but paths are not allowed to be empty")
    }
}

impl Error for EmptyRouteError {}

impl From<SourceRoute> for Path {
    fn from(value: SourceRoute) -> Self {
        Path::from(value.ids)
    }
}

impl From<Path> for SourceRoute {
    fn from(path: Path) -> Self {
        Self {
            ids: Vec::from(path),
            progress: 0,
        }
    }
}
