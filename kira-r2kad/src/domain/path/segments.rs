//! Splitting of paths into the path segments used by the forwarding tier.
//!
//! Every path to a contact is represented by up to two [PathIds](crate::domain::PathId),
//! one per _path segment_ (see section 3.10 of the [KIRA draft][kira_draft]):
//!
//! | hops `L` | first segment | second segment |
//! |----------|---------------|----------------|
//! | 1 – 3    | whole path    | –              |
//! | 4        | 2 hops        | 2 hops         |
//! | 5        | 3 hops        | 2 hops         |
//! | ≥ 6      | `L` − 3 hops  | 3 hops         |
//!
//! The PathId of the second segment is calculated including the last node of the first
//! segment, as this node maps it to the PathId of the remaining path.
//!
//! This is the [two-segment scheme](PathSegmentation::TwoSegments). Forwarding
//! implementations that can't carry two PathIds use the
//! [one-segment scheme](PathSegmentation::OneSegment), where the whole path is a single
//! segment. All nodes of a network have to use the same scheme.
//!
//! [kira_draft]: https://datatracker.ietf.org/doc/draft-bless-rtgwg-kira/

use crate::domain::{
    NodeId,
    Path,
    VICINITY_RADIUS,
};

/// Longest path in hops that is covered by a single path segment.
///
/// PathIds of these paths are precomputed by all nodes on the path.
const MAX_SINGLE_SEGMENT_HOPS: usize = VICINITY_RADIUS;

/// Shortest path in hops whose second segment has [MAX_SINGLE_SEGMENT_HOPS] hops.
///
/// PathIds of these paths have to be installed explicitly in some of the nodes on the path.
const MIN_SETUP_HOPS: usize = 2 * VICINITY_RADIUS;

/// How paths to contacts are split into path segments.
///
/// All nodes of a network have to use the same scheme, as the nodes on a path derive the
/// PathIds they have to know from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PathSegmentation {
    /// The whole path is a single segment with one PathId.
    ///
    /// Paths of more than three hops require an explicit path setup in all nodes that are
    /// more than two hops away from the end of the path.
    OneSegment,
    /// Paths of more than three hops are split into two segments (see [PathSegments]).
    ///
    /// Only paths of at least six hops require an explicit path setup.
    #[default]
    TwoSegments,
}

/// The path segments of a [Path] to a contact.
///
/// The [Path] doesn't include the node the path starts at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathSegments<'a> {
    path: &'a [NodeId],
    first_hops: usize,
    segmentation: PathSegmentation,
}

impl<'a> PathSegments<'a> {
    /// Splits the path into its segments.
    pub fn split(path: &'a Path, segmentation: PathSegmentation) -> Self {
        Self::split_ids(path.as_ref(), segmentation)
    }

    /// Splits the path given by its [NodeId]s into its segments.
    ///
    /// # Panics
    ///
    /// If the path is empty.
    pub fn split_ids(path: &'a [NodeId], segmentation: PathSegmentation) -> Self {
        assert!(!path.is_empty(), "paths are not allowed to be empty");
        Self {
            path,
            first_hops: Self::first_segment_hops(path.len(), segmentation),
            segmentation,
        }
    }

    /// Number of hops of the first segment of a path with the given number of hops.
    pub fn first_segment_hops(hops: usize, segmentation: PathSegmentation) -> usize {
        if segmentation == PathSegmentation::OneSegment || hops <= MAX_SINGLE_SEGMENT_HOPS {
            hops
        } else if hops < MIN_SETUP_HOPS {
            hops - (MAX_SINGLE_SEGMENT_HOPS - 1)
        } else {
            hops - MAX_SINGLE_SEGMENT_HOPS
        }
    }

    /// Returns if paths with the given number of hops need an explicit path setup,
    /// i.e., some nodes on the path haven't precomputed the required PathIds.
    pub fn hops_require_setup(hops: usize, segmentation: PathSegmentation) -> bool {
        match segmentation {
            PathSegmentation::OneSegment => hops > MAX_SINGLE_SEGMENT_HOPS,
            PathSegmentation::TwoSegments => hops >= MIN_SETUP_HOPS,
        }
    }

    /// Returns if the path needs an explicit path setup.
    pub fn requires_setup(&self) -> bool {
        Self::hops_require_setup(self.path.len(), self.segmentation)
    }

    /// Nodes of the first segment.
    ///
    /// Its PathId is the outgoing PathId at the node the path starts at.
    pub fn first(&self) -> &'a [NodeId] {
        &self.path[..self.first_hops]
    }

    /// Nodes of the second segment starting with the last node of the first segment.
    ///
    /// Its PathId is the incoming PathId at the last node of the first segment.
    pub fn second(&self) -> Option<&'a [NodeId]> {
        (self.first_hops < self.path.len()).then(|| &self.path[self.first_hops - 1..])
    }

    /// Nodes of the incoming PathId at the node with the given index in the path.
    ///
    /// These are the remaining nodes of the segment starting with the node itself.
    /// The last node of the first segment is the first node of the second segment.
    ///
    /// Returns [None] if the index is not in the path.
    pub fn in_path_at(&self, index: usize) -> Option<&'a [NodeId]> {
        if index >= self.path.len() {
            None
        } else if index + 1 < self.first_hops {
            Some(&self.path[index..self.first_hops])
        } else {
            Some(&self.path[index..])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(hops: usize) -> Vec<NodeId> {
        (1..=hops).map(|i| NodeId::with_lsb(i as u8)).collect()
    }

    /// Returns the lengths in nodes of the first and second segment.
    fn segment_sizes(hops: usize) -> (usize, Option<usize>) {
        let ids = ids(hops);
        let segments = PathSegments::split_ids(&ids, PathSegmentation::TwoSegments);
        (segments.first().len(), segments.second().map(<[_]>::len))
    }

    #[test]
    fn short_paths_have_a_single_segment() {
        assert_eq!(segment_sizes(1), (1, None));
        assert_eq!(segment_sizes(2), (2, None));
        assert_eq!(segment_sizes(3), (3, None));
    }

    #[test]
    fn paths_of_four_and_five_hops_have_second_segment_of_two_hops() {
        // the second segment includes the last node of the first segment
        assert_eq!(segment_sizes(4), (2, Some(3)));
        assert_eq!(segment_sizes(5), (3, Some(3)));
    }

    #[test]
    fn longer_paths_have_second_segment_of_three_hops() {
        for hops in 6..=9 {
            assert_eq!(segment_sizes(hops), (hops - 3, Some(4)));
        }
    }

    #[test]
    fn only_paths_of_at_least_six_hops_require_setup() {
        for hops in 1..=5 {
            assert!(
                !PathSegments::split_ids(&ids(hops), PathSegmentation::TwoSegments)
                    .requires_setup()
            );
        }
        for hops in 6..=9 {
            assert!(
                PathSegments::split_ids(&ids(hops), PathSegmentation::TwoSegments).requires_setup()
            );
        }
    }

    #[test]
    fn segments_of_seven_hop_path() {
        // Figure 3 of the draft: X -> A Q M Z C E T
        let ids = ids(7);
        let segments = PathSegments::split_ids(&ids, PathSegmentation::TwoSegments);
        assert_eq!(segments.first(), &ids[..4]); // A Q M Z
        assert_eq!(segments.second(), Some(&ids[3..])); // Z C E T
    }

    #[test]
    fn in_paths_of_seven_hop_path() {
        let ids = ids(7);
        let segments = PathSegments::split_ids(&ids, PathSegmentation::TwoSegments);
        assert_eq!(segments.in_path_at(0), Some(&ids[0..4])); // A Q M Z
        assert_eq!(segments.in_path_at(1), Some(&ids[1..4])); // Q M Z
        assert_eq!(segments.in_path_at(2), Some(&ids[2..4])); // M Z
        assert_eq!(segments.in_path_at(3), Some(&ids[3..])); // Z C E T
        assert_eq!(segments.in_path_at(4), Some(&ids[4..])); // C E T
        assert_eq!(segments.in_path_at(6), Some(&ids[6..])); // T
        assert_eq!(segments.in_path_at(7), None);
    }

    #[test]
    fn in_paths_of_single_segment_path() {
        let ids = ids(3);
        let segments = PathSegments::split_ids(&ids, PathSegmentation::TwoSegments);
        assert_eq!(segments.in_path_at(0), Some(&ids[..]));
        assert_eq!(segments.in_path_at(2), Some(&ids[2..]));
    }

    #[test]
    fn one_segment_scheme_never_splits() {
        for hops in 1..=9 {
            let ids = ids(hops);
            let segments = PathSegments::split_ids(&ids, PathSegmentation::OneSegment);
            assert_eq!(segments.first(), &ids[..]);
            assert_eq!(segments.second(), None);
            assert_eq!(segments.requires_setup(), hops > 3);
            // every node needs the PathId of the remaining path
            assert_eq!(segments.in_path_at(hops - 1), Some(&ids[hops - 1..]));
            assert_eq!(segments.in_path_at(0), Some(&ids[..]));
        }
    }
}
