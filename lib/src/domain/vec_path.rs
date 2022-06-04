use std::fmt::{Display, Formatter};
use std::ops::Index;
use std::slice::SliceIndex;

use crate::domain::Path;

/// A Path of [Id]s.
///
/// This implementation is backed by a [Vec].
#[derive(Debug)]
pub struct VecPath<I> {
    inner: Vec<I>,
}

/// Converts a Vector of [Id]s to a Path.
///
/// It may be advised to shrink the [Vec] to its length
/// with [Vec::shrink_to_fit]
impl<I> From<Vec<I>> for VecPath<I> {
    fn from(vec: Vec<I>) -> Self {
        Self { inner: vec }
    }
}

impl<I: Clone> From<&[I]> for VecPath<I> {
    fn from(slice: &[I]) -> Self {
        Self {
            inner: Vec::from(slice),
        }
    }
}

impl<I, const SIZE: usize> From<[I; SIZE]> for VecPath<I> {
    fn from(raw: [I; SIZE]) -> Self {
        Self {
            inner: Vec::from(raw),
        }
    }
}

impl<I> VecPath<I> {
    /// Creates an empty [VecPath].
    pub const fn empty() -> Self {
        VecPath { inner: Vec::new() }
    }

    /// Returns if the path is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns the number of [Id]s in this path.
    pub fn len(&self) -> usize {
        self.inner.len()
    }
}

/// [Clone] is only implemented, if the Id Type also implements [Clone].
impl<I: Clone> Clone for VecPath<I> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

// ============ Formatting ============

impl<I: Display> Display for VecPath<I> {
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

impl<I: Eq> PartialEq<Self> for VecPath<I> {
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

impl<I: Eq> Eq for VecPath<I> {}

// ============ Indexing ============

impl<I, Idx> Index<Idx> for VecPath<I>
where
    Idx: SliceIndex<[I]>,
{
    type Output = Idx::Output;

    fn index(&self, index: Idx) -> &Self::Output {
        self.inner.index(index)
    }
}

// ============ Actual Path Functionality ============

impl<I: Clone + Display + Eq> Path for VecPath<I> {
    fn reverse(&mut self) {
        self.inner.reverse();
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::Path;
    use crate::domain::{NodeId, VecPath};

    #[test]
    fn index_smoke_test() {
        let indexed = VecPath::from([
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
        let path = VecPath::from([
            NodeId::<1>::from([15]),
            NodeId::<1>::from([14]),
            NodeId::<1>::from([13]),
        ]);

        assert_eq!(
            path,
            VecPath::from([
                NodeId::<1>::from([15]),
                NodeId::<1>::from([14]),
                NodeId::<1>::from([13]),
            ])
        );
        assert_ne!(
            path,
            VecPath::from([
                NodeId::<1>::from([13]),
                NodeId::<1>::from([14]),
                NodeId::<1>::from([15]),
            ])
        )
    }

    #[test]
    fn reverse() {
        let path = VecPath::from([
            NodeId::<1>::from([15]),
            NodeId::<1>::from([14]),
            NodeId::<1>::from([13]),
        ]);
        let mut reversed = path.clone();
        reversed.reverse();

        assert_ne!(path, reversed);
        assert_eq!(
            reversed,
            VecPath::from([
                NodeId::<1>::from([13]),
                NodeId::<1>::from([14]),
                NodeId::<1>::from([15]),
            ])
        )
    }
}
