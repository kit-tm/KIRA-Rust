use std::fmt::{Display, Formatter};
use std::ops::Index;
use std::slice::SliceIndex;

use crate::domain::Id;

/// A Path of [Id]s.
///
/// As Paths don't have a fixed length, a [Vec] has to be used here.
#[derive(Debug, Eq)]
pub struct Path<I: Id> {
    inner: Vec<I>,
}

/// Converts a Vector of [Id]s to a Path.
///
/// It may be advised to shrink the [Vec] to its length
/// with [Vec::shrink_to_fit]
impl<I: Id> From<Vec<I>> for Path<I> {
    fn from(vec: Vec<I>) -> Self {
        Self { inner: vec }
    }
}

impl<I: Id> From<&[I]> for Path<I> {
    fn from(slice: &[I]) -> Self {
        Self {
            inner: Vec::from(slice),
        }
    }
}

impl<I: Id, const SIZE: usize> From<[I; SIZE]> for Path<I> {
    fn from(raw: [I; SIZE]) -> Self {
        Self {
            inner: Vec::from(raw),
        }
    }
}

impl<I: Id> Path<I> {
    /// Creates an empty [Path].
    pub const fn empty() -> Self {
        Path { inner: Vec::new() }
    }

    /// Returns if the path is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns the number of [Id]s in this path.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Reverses the [Path] in-place.
    pub fn reverse(&mut self) {
        self.inner.reverse();
    }
}

/// [Clone] is only implemented, if the Id Type also implements [Clone].
impl<I: Id + Clone> Clone for Path<I> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

// ============ Formatting ============

impl<I: Id + Display> Display for Path<I> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "<")?;
        let length = self.inner.len();
        for (index, id) in self.inner.iter().enumerate() {
            write!(f, "{}", id)?;
            if index < length - 1 {
                write!(f, ",")?;
            }
        }
        write!(f, ">")
    }
}

// ============ Equality ============

impl<I: Id> PartialEq for Path<I> {
    fn eq(&self, other: &Self) -> bool {
        if self.len() != other.len() {
            return false;
        }
        for (left, right) in self.inner.iter().zip(other.inner.iter()) {
            if left != right {
                return false;
            }
        }
        true
    }
}

// ============ Indexing ============

impl<I, Idx> Index<Idx> for Path<I>
where
    I: Id,
    Idx: SliceIndex<[I]>,
{
    type Output = Idx::Output;

    fn index(&self, index: Idx) -> &Self::Output {
        self.inner.index(index)
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::NodeId;
    use crate::domain::Path;

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
