use derive_more::Error;
use derive_more::with_trait::Display;

use std::ops::Index;
use std::slice::SliceIndex;

use crate::domain::{NodeId, Timestamp, hasher::Hasher};
use std::sync::OnceLock;

use super::Link;

pub mod cycle_remover;
pub mod in_order_cycle_remover;
pub mod pathcollection;
pub mod shortest_first_path_simplifier;
pub mod simplifier;

/// this is a static variable that automatically gets initialized on its first use
/// it represents a random NodeID that serves to prevent route flapping
pub struct AnchorNodeId;

impl AnchorNodeId {
    pub fn get(&mut self) -> &'static NodeId {
        static INSTANCE: OnceLock<NodeId> = OnceLock::new();
        INSTANCE.get_or_init(NodeId::random)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum PathState {
    #[default]
    Undefined, // initial state, path state not yet defined
    Valid,    // path is valid (has been validated)
    Faulty,   // path not usable but rediscovery is initiated
    Checking, // path probably usable, but needs to be validated (e.g., for a proposed path)
}

/// A Path of [NodeId]s.
///
/// This implementation is backed by a [Vec].
///
/// # Invariant
///
/// A valid [Path] is not empty at any time as it always contains the NodeId of the destination node at the end
/// Therefore some methods panic or return errors when constructing empty [Path]s.
/// The last_validated timestamp is the instant when the path was successfully validated by a PathProbe or invalidated by an error
/// The last_path_refresh timestamp is the instant when the path was successfully refreshed
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct Path {
    ids: Vec<NodeId>,
    #[cfg_attr(feature = "serde", serde(skip))]
    path_state: PathState,
    #[cfg_attr(feature = "serde", serde(skip))]
    last_validated: Option<Timestamp>, // update for last validation or invalidation
    #[cfg_attr(feature = "serde", serde(skip))]
    last_path_refresh: Option<Timestamp>,
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

        Ok(Self {
            ids: value,
            path_state: Default::default(),
            last_validated: None,
            last_path_refresh: None,
        })
    }
}

#[derive(Debug, Eq, PartialEq, Display, Error)]
#[display("Empty paths are not allowed")]
pub struct EmptyPathError;

impl TryFrom<&[NodeId]> for Path {
    type Error = EmptyPathError;
    fn try_from(slice: &[NodeId]) -> Result<Self, Self::Error> {
        if slice.is_empty() {
            return Err(EmptyPathError);
        }

        Ok(Self {
            ids: Vec::from(slice),
            path_state: Default::default(),
            last_validated: None,
            last_path_refresh: None,
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
            path_state: Default::default(),
            last_validated: None,
            last_path_refresh: None,
        }
    }
}

impl From<NodeId> for Path {
    fn from(raw: NodeId) -> Self {
        Self {
            ids: vec![raw],
            path_state: Default::default(),
            last_validated: None,
            last_path_refresh: None,
        }
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

        Ok(Path {
            ids: vec,
            path_state: Default::default(),
            last_validated: None,
            last_path_refresh: None,
        })
    }
}

impl Path {
    // set path to None and clear everything else
    pub fn clear(&mut self) {
        self.ids.clear();
        self.path_state = Default::default();
        self.last_validated = None;
        self.last_path_refresh = None;
    }

    /// Reverses the [Path] in-place.
    pub fn reverse(&mut self) {
        self.ids.reverse();
    }
    /// Size of the [Path] in numbers of Nodes.
    ///
    /// Is usually always > 0 as Path has to contain the [Contact](crate::domain::Contact)s NodeId at the
    /// end.
    pub fn size(&self) -> usize {
        debug_assert!(
            self.path_state != PathState::Undefined
                || (self.path_state == PathState::Undefined && !self.ids.is_empty())
        );
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
            .any(|(first, second)| {
                (first == link.first() && second == link.second())
                    || (first == link.second() && second == link.first())
            })
    }
    /// Returns the first entry in the [Path].
    pub fn first(&self) -> &NodeId {
        self.ids
            .first()
            .expect("Invalid access to first element on empty path")
    }
    /// Returns the second entry in the [Path].
    pub fn second(&self) -> Option<&NodeId> {
        self.ids.get(1)
    }
    /// Returns the last entry in the [Path].
    pub fn last(&self) -> &NodeId {
        self.ids
            .last()
            .expect("Invalid access to last element on empty path")
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

    /// Check if the [Path] starts with the [NodeId]s in the iterator.
    pub fn starts_with<'a, I>(&self, iter: I) -> bool
    where
        I: IntoIterator<Item = &'a NodeId>,
    {
        let mut iter = iter.into_iter();
        for own_id in self.ids.iter() {
            match iter.next() {
                Some(other_id) => {
                    if own_id != other_id {
                        return false;
                    }
                }
                None => return true,
            }
        }

        iter.next().is_none()
    }

    /// returns true if this path is better (shorter or same length but closer to AnchorNodeId)
    pub fn is_better_than(&self, other_path: &Path) -> bool {
        debug_assert!(self.last() == other_path.last()); // paths should have the same destination
        self.ids.len() < other_path.ids.len()
            || (self.ids.len() == other_path.ids.len()
                && (self.path_hasher().hash(&self.ids) ^ AnchorNodeId.get())
                    < self.path_hasher().hash(&other_path.ids) ^ AnchorNodeId.get())
    }

    /// returns true if this path is same
    pub fn is_same_path_as(&self, other_path: &Path) -> bool {
        self.ids == other_path.ids
    }

    /// get path state
    pub fn get_state(&self) -> PathState {
        self.path_state
    }

    /// set path state
    pub fn set_state(&mut self, new_state: PathState) {
        if new_state == PathState::Undefined {
            panic!("Path State MUST never be set to Undefined");
        } else {
            self.path_state = new_state;
        }
    }

    pub fn is_valid(&self) -> bool {
        matches!(self.path_state, PathState::Valid)
    }

    pub fn is_faulty(&self) -> bool {
        matches!(self.path_state, PathState::Faulty)
    }

    pub fn is_checking(&self) -> bool {
        matches!(self.path_state, PathState::Checking)
    }

    /// invalidate current path
    pub fn invalidate(&mut self) {
        self.set_state(PathState::Faulty);
        self.update_last_validated();
    }

    /// returns true if path was validated before the given instant (or never)
    pub fn is_older_than(&self, ts: &Timestamp) -> bool {
        match self.last_validated {
            Some(last_ts) => last_ts < *ts,
            None => true,
        }
    }

    pub fn get_last_validated(&self) -> Option<&Timestamp> {
        self.last_validated.as_ref()
    }

    pub fn get_last_path_refresh(&self) -> Option<&Timestamp> {
        self.last_path_refresh.as_ref()
    }

    pub fn update_last_validated(&mut self) {
        // instant should be CurrentRuntime::now()
        self.last_validated = Some(Timestamp::now());
    }

    pub fn unset_last_validated(&mut self) {
        // instant should be CurrentRuntime::now()
        self.last_validated = None;
    }

    pub fn update_last_path_refresh(&mut self, instant: Timestamp) {
        // instant should be CurrentRuntime::now()
        self.last_path_refresh = Some(instant);
    }

    pub fn unset_last_path_refresh(&mut self) {
        // instant should be CurrentRuntime::now()
        self.last_validated = None;
    }

    fn path_hasher(&self) -> Hasher {
        Hasher::default()
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
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<")?;
        write!(
            f,
            "{}",
            self.ids
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(",")
        )?;
        write!(f, ">")
    }
}

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

impl<'a> IntoIterator for &'a Path {
    type Item = &'a NodeId;
    type IntoIter = std::slice::Iter<'a, NodeId>;

    fn into_iter(self) -> Self::IntoIter {
        self.ids.iter()
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::{NodeId, Path};

    #[test]
    fn index_smoke_test() {
        let indexed = Path::from([
            NodeId::from(0u128),
            NodeId::from(1u128),
            NodeId::from(2u128),
        ]);

        assert_eq!(&indexed[1..], [NodeId::from(1u128), NodeId::from(2u128)]);
        assert_eq!(
            &indexed[..],
            [
                NodeId::from(0u128),
                NodeId::from(1u128),
                NodeId::from(2u128)
            ]
        );
    }

    #[test]
    fn equality() {
        let path = Path::from([
            NodeId::from(15u128),
            NodeId::from(14u128),
            NodeId::from(13u128),
        ]);

        assert_eq!(
            path,
            Path::from([
                NodeId::from(15u128),
                NodeId::from(14u128),
                NodeId::from(13u128),
            ])
        );
        assert_ne!(
            path,
            Path::from([
                NodeId::from(13u128),
                NodeId::from(14u128),
                NodeId::from(15u128),
            ])
        )
    }

    #[test]
    fn reverse() {
        let path = Path::from([
            NodeId::from(15u128),
            NodeId::from(14u128),
            NodeId::from(13u128),
        ]);
        let mut reversed = path.clone();
        reversed.reverse();

        assert_ne!(path, reversed);
        assert_eq!(
            reversed,
            Path::from([
                NodeId::from(13u128),
                NodeId::from(14u128),
                NodeId::from(15u128),
            ])
        )
    }

    #[test]
    fn remove_in() {
        let mut path = Path::from([
            NodeId::from(1u128),
            NodeId::from(2u128),
            NodeId::from(3u128),
            NodeId::from(4u128),
            NodeId::from(5u128),
        ]);

        path.remove_in(1, 3);

        assert_eq!(
            path,
            Path::from([
                NodeId::from(1u128),
                NodeId::from(4u128),
                NodeId::from(5u128),
            ])
        );

        path.remove_in(1, 2);

        assert_eq!(
            path,
            Path::from([NodeId::from(1u128), NodeId::from(5u128),])
        );
    }

    #[test]
    fn replace_interval() {
        let mut path = Path::from([
            NodeId::from(1u128),
            NodeId::from(2u128),
            NodeId::from(3u128),
            NodeId::from(4u128),
            NodeId::from(5u128),
            NodeId::from(6u128),
        ]);

        path.replace_interval(1, 3, [NodeId::ZERO, NodeId::ONE]);

        assert_eq!(
            path,
            Path::from([
                NodeId::from(1u128),
                NodeId::from(0u128),
                NodeId::from(1u128),
                NodeId::from(5u128),
                NodeId::from(6u128),
            ])
        );
    }

    #[test]
    fn replace_interval_start() {
        let mut path = Path::from([
            NodeId::from(1u128),
            NodeId::from(2u128),
            NodeId::from(3u128),
            NodeId::from(4u128),
            NodeId::from(5u128),
            NodeId::from(6u128),
        ]);

        path.replace_interval(0, 3, [NodeId::ZERO, NodeId::ONE]);

        assert_eq!(
            path,
            Path::from([
                NodeId::from(0u128),
                NodeId::from(1u128),
                NodeId::from(5u128),
                NodeId::from(6u128),
            ])
        );
    }

    #[test]
    fn replace_interval_end() {
        let mut path = Path::from([
            NodeId::from(1u128),
            NodeId::from(2u128),
            NodeId::from(3u128),
            NodeId::from(4u128),
            NodeId::from(5u128),
            NodeId::from(6u128),
        ]);

        path.replace_interval(3, 5, [NodeId::ZERO, NodeId::ONE]);

        assert_eq!(
            path,
            Path::from([
                NodeId::from(1u128),
                NodeId::from(2u128),
                NodeId::from(3u128),
                NodeId::from(0u128),
                NodeId::from(1u128),
            ])
        );
    }
}
