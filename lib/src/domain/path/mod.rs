use std::error::Error;
use std::fmt::{Display, Formatter};
use std::ops::Index;
use std::slice::SliceIndex;

use crate::domain::NodeId;

use super::Link;

pub mod cycle_remover;
pub mod in_order_cycle_remover;
pub mod shortest_first_path_simplifier;
pub mod simplifier;

/// A Path of [NodeId]s.
///
/// This implementation is backed by a [Vec].
///
/// # Invariant
///
/// A valid Path is not empty at any time.
/// Therefore the methods panic or return errors when constructing empty [Path]s.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct Path {
    ids: Vec<NodeId>,
}

/// Converts a Vector of [NodeId]s to a Path.
///
/// It may be advised to shrink the [Vec] to its length
/// with [Vec::shrink_to_fit]
impl TryFrom<Vec<NodeId>> for Path {
    type Error = EmptyPathError;

    fn try_from(value: Vec<NodeId>) -> Result<Self, Self::Error> {
        if value.is_empty() {
            return Err(EmptyPathError);
        }

        Ok(Self { ids: value })
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct EmptyPathError;

impl Display for EmptyPathError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Empty paths are not allowed")
    }
}

impl Error for EmptyPathError {}

impl TryFrom<&[NodeId]> for Path {
    type Error = EmptyPathError;
    fn try_from(slice: &[NodeId]) -> Result<Self, Self::Error> {
        if slice.is_empty() {
            return Err(EmptyPathError);
        }

        Ok(Self {
            ids: Vec::from(slice),
        })
    }
}

// NOTE: As soon as const generic where conditions are stable, use here.
impl<const PATH_SIZE: usize> From<[NodeId; PATH_SIZE]> for Path {
    fn from(raw: [NodeId; PATH_SIZE]) -> Self {
        if PATH_SIZE == 0 {
            panic!("{}", EmptyPathError);
        }
        Self {
            ids: Vec::from(raw),
        }
    }
}

impl From<NodeId> for Path {
    fn from(raw: NodeId) -> Self {
        Self { ids: vec![raw] }
    }
}

impl From<Path> for Vec<NodeId> {
    fn from(path: Path) -> Self {
        path.ids
    }
}

impl FromIterator<NodeId> for Result<Path, EmptyPathError> {
    fn from_iter<T: IntoIterator<Item = NodeId>>(iter: T) -> Self {
        let vec = Vec::from_iter(iter);

        if vec.is_empty() {
            return Err(EmptyPathError);
        }

        Ok(Path { ids: vec })
    }
}

impl Path {
    /// Reverses the [Path] in-place.
    pub fn reverse(&mut self) {
        self.ids.reverse();
    }
    /// Size of the [Path] in numbers of Nodes.
    ///
    /// Is always > 0 as Path has to contain the [Contact]s NodeId at the end.
    pub fn size(&self) -> usize {
        self.ids.len()
    }
    /// Returns if the [Path] contains the [NodeId].
    pub fn contains(&self, id: &NodeId) -> bool {
        self.ids.contains(id)
    }
    /// Returns if the [Path] contains the [Link].
    pub fn contains_link(&self, link: &Link) -> bool {
        self.ids
            .iter()
            .zip(self.ids.iter().skip(1))
            .any(|(first, second)| first == &link.0 && second == &link.1)
    }
    /// Returns the first entry in the [Path].
    pub fn first(&self) -> &NodeId {
        self.ids
            .first()
            .expect("Invalid state. Empty path constructed")
    }
    /// Returns the last entry in the [Path].
    pub fn last(&self) -> &NodeId {
        self.ids
            .last()
            .expect("Invalid state. Empty path constructed")
    }
    /// Pushs a [NodeId] to the end of the [Path].
    pub fn push(&mut self, id: NodeId) {
        self.ids.push(id);
    }
    /// Remove all entries inside the interval [start_index, end_index).
    /// Note that the end_index is excluded.
    fn remove_in(&mut self, start_index: usize, end_index: usize) {
        assert!(end_index > start_index);
        assert!(end_index <= self.ids.len());
        let num = end_index - start_index;
        for _ in 0..num {
            self.ids.remove(start_index);
        }
    }

    /// Replaces an interval in the [Path] with other [NodeId]s.
    pub fn replace_interval<P: IntoIterator<Item = NodeId>>(
        &mut self,
        start_index: usize,
        end_index: usize,
        part: P,
    ) {
        if start_index > end_index {
            panic!("Start index has to be <= end index");
        }
        // Remove all elements to replace
        for _ in 0..(end_index - start_index + 1) {
            self.ids.remove(start_index);
        }
        for (i, id) in part.into_iter().enumerate() {
            self.ids.insert(start_index + i, id);
        }
    }

    /// Returns an [Iterator] over its elements from start
    /// to end.
    pub fn iter(&self) -> impl Iterator<Item = &NodeId> {
        self.ids.iter()
    }
}

impl Extend<NodeId> for Path {
    fn extend<T: IntoIterator<Item = NodeId>>(&mut self, iter: T) {
        self.ids.extend(iter);
    }
}

// ============ Conversions ============

impl AsRef<[NodeId]> for Path {
    fn as_ref(&self) -> &[NodeId] {
        self.ids.as_ref()
    }
}

// ============ Formatting ============

impl Display for Path {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "<")?;
        let length = self.ids.len();
        for (index, id) in self.ids.iter().enumerate() {
            write!(f, "{}", id)?;
            if index < length - 1 {
                write!(f, ",")?;
            }
        }
        write!(f, ">")
    }
}

// ============ Equality ============

impl PartialEq<Self> for Path {
    fn eq(&self, other: &Self) -> bool {
        if self.size() != other.size() {
            return false;
        }
        for (left, right) in self.ids.iter().zip(other.ids.iter()) {
            if left != right {
                return false;
            }
        }
        true
    }
}

impl Eq for Path {}

// ============ Indexing ============

impl<Idx> Index<Idx> for Path
where
    Idx: SliceIndex<[NodeId]>,
{
    type Output = Idx::Output;

    fn index(&self, index: Idx) -> &Self::Output {
        self.ids.index(index)
    }
}

// ============ Iteration ============

impl IntoIterator for Path {
    type Item = NodeId;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.ids.into_iter()
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::{NodeId, Path};

    #[test]
    fn index_smoke_test() {
        let indexed = Path::from([
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2]),
        ]);

        assert_eq!(
            &indexed[1..],
            [
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2])
            ]
        );
        assert_eq!(
            &indexed[..],
            [
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2])
            ]
        );
    }

    #[test]
    fn equality() {
        let path = Path::from([
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 15]),
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 14]),
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 13]),
        ]);

        assert_eq!(
            path,
            Path::from([
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 15]),
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 14]),
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 13]),
            ])
        );
        assert_ne!(
            path,
            Path::from([
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 13]),
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 14]),
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 15]),
            ])
        )
    }

    #[test]
    fn reverse() {
        let path = Path::from([
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 15]),
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 14]),
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 13]),
        ]);
        let mut reversed = path.clone();
        reversed.reverse();

        assert_ne!(path, reversed);
        assert_eq!(
            reversed,
            Path::from([
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 13]),
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 14]),
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 15]),
            ])
        )
    }

    #[test]
    fn remove_in() {
        let mut path = Path::from([
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2]),
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3]),
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4]),
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5]),
        ]);

        path.remove_in(1, 3);

        assert_eq!(
            path,
            Path::from([
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4]),
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5]),
            ])
        );

        path.remove_in(1, 2);

        assert_eq!(
            path,
            Path::from([
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5]),
            ])
        );
    }

    #[test]
    fn replace_interval() {
        let mut path = Path::from([
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2]),
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3]),
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4]),
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5]),
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 6]),
        ]);

        path.replace_interval(1, 3, [NodeId::zero(), NodeId::one()]);

        assert_eq!(
            path,
            Path::from([
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5]),
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 6]),
            ])
        );
    }

    #[test]
    fn replace_interval_start() {
        let mut path = Path::from([
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2]),
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3]),
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4]),
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5]),
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 6]),
        ]);

        path.replace_interval(0, 3, [NodeId::zero(), NodeId::one()]);

        assert_eq!(
            path,
            Path::from([
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5]),
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 6]),
            ])
        );
    }

    #[test]
    fn replace_interval_end() {
        let mut path = Path::from([
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2]),
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3]),
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4]),
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5]),
            NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 6]),
        ]);

        path.replace_interval(3, 5, [NodeId::zero(), NodeId::one()]);

        assert_eq!(
            path,
            Path::from([
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2]),
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3]),
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
                NodeId::from([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]),
            ])
        );
    }
}
