use std::fmt::{Display, Formatter};
use std::ops::Index;
use std::slice::SliceIndex;

use crate::domain::{NodeId, DEFAULT_ID_SIZE};

/// A Path of [NodeId]s.
///
/// This implementation is backed by a [Vec].
#[derive(Debug, Clone)]
pub struct Path<const ID_SIZE: usize = DEFAULT_ID_SIZE> {
    ids: Vec<NodeId<ID_SIZE>>,
}

/// Converts a Vector of [NodeId]s to a Path.
///
/// It may be advised to shrink the [Vec] to its length
/// with [Vec::shrink_to_fit]
impl<const ID_SIZE: usize> From<Vec<NodeId<ID_SIZE>>> for Path<ID_SIZE> {
    fn from(vec: Vec<NodeId<ID_SIZE>>) -> Self {
        Self { ids: vec }
    }
}

impl<const ID_SIZE: usize> From<&[NodeId<ID_SIZE>]> for Path<ID_SIZE> {
    fn from(slice: &[NodeId<ID_SIZE>]) -> Self {
        Self {
            ids: Vec::from(slice),
        }
    }
}

impl<const PATH_SIZE: usize, const ID_SIZE: usize> From<[NodeId<ID_SIZE>; PATH_SIZE]>
    for Path<ID_SIZE>
{
    fn from(raw: [NodeId<ID_SIZE>; PATH_SIZE]) -> Self {
        Self {
            ids: Vec::from(raw),
        }
    }
}

impl<const ID_SIZE: usize> Path<ID_SIZE> {
    /// Creates an empty [Path].
    pub const fn empty() -> Self {
        Path { ids: Vec::new() }
    }

    /// Reverses the [Path] in-place.
    pub fn reverse(&mut self) {
        self.ids.reverse();
    }
    /// Length of the [Path] in numbers of Nodes.
    pub fn len(&self) -> usize {
        self.ids.len()
    }
    /// Returns if the Path is empty.
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
}

// ============ Formatting ============

impl<const ID_SIZE: usize> Display for Path<ID_SIZE> {
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

impl<const ID_SIZE: usize> PartialEq<Self> for Path<ID_SIZE> {
    fn eq(&self, other: &Self) -> bool {
        if self.len() != other.len() {
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

impl<const ID_SIZE: usize> Eq for Path<ID_SIZE> {}

// ============ Indexing ============

impl<Idx, const ID_SIZE: usize> Index<Idx> for Path<ID_SIZE>
where
    Idx: SliceIndex<[NodeId<ID_SIZE>]>,
{
    type Output = Idx::Output;

    fn index(&self, index: Idx) -> &Self::Output {
        self.ids.index(index)
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::{NodeId, Path};

    #[test]
    fn index_smoke_test() {
        let indexed = Path::from([
            NodeId::<1>::from([0]),
            NodeId::<1>::from([1]),
            NodeId::<1>::from([2]),
        ]);

        assert_eq!(
            &indexed[1..],
            [NodeId::<1>::from([1]), NodeId::<1>::from([2])]
        );
        assert_eq!(
            &indexed[..],
            [
                NodeId::<1>::from([0]),
                NodeId::<1>::from([1]),
                NodeId::<1>::from([2])
            ]
        );
    }

    #[test]
    fn equality() {
        let path = Path::from([
            NodeId::<1>::from([15]),
            NodeId::<1>::from([14]),
            NodeId::<1>::from([13]),
        ]);

        assert_eq!(
            path,
            Path::from([
                NodeId::<1>::from([15]),
                NodeId::<1>::from([14]),
                NodeId::<1>::from([13]),
            ])
        );
        assert_ne!(
            path,
            Path::from([
                NodeId::<1>::from([13]),
                NodeId::<1>::from([14]),
                NodeId::<1>::from([15]),
            ])
        )
    }

    #[test]
    fn reverse() {
        let path = Path::from([
            NodeId::<1>::from([15]),
            NodeId::<1>::from([14]),
            NodeId::<1>::from([13]),
        ]);
        let mut reversed = path.clone();
        reversed.reverse();

        assert_ne!(path, reversed);
        assert_eq!(
            reversed,
            Path::from([
                NodeId::<1>::from([13]),
                NodeId::<1>::from([14]),
                NodeId::<1>::from([15]),
            ])
        )
    }
}
