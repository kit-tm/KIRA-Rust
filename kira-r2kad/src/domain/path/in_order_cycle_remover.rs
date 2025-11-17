use crate::domain::cycle_remover::PathCycleRemover;
use crate::domain::Path;

/// This algorithm iterates the [Path] from first to last [NodeId](crate::domain::node_id::NodeId).
///
/// When searching for duplicates it starts from the back to remove the
/// longest cycle for the given [NodeId](crate::domain::node_id::NodeId).
#[derive(Debug)]
pub struct InOrderCycleRemover;

impl PathCycleRemover for InOrderCycleRemover {
    fn remove_cycles_in_place(&mut self, path: &mut Path) {
        if path.size() < 2 {
            return;
        }

        let mut iter = path.ids.clone().into_iter();
        let mut index = 0;
        while let Some(id) = iter.next() {
            if index == path.ids.len() {
                // skipped enough to have reached the last element
                break;
            }
            let remaining_ids = &path.ids[(index + 1)..];
            // We search for the longest cycle, so we get position from the back
            let pos = remaining_ids
                .iter()
                .enumerate()
                .rev()
                .find_map(|(index, duplicate_id)| {
                    if duplicate_id == &id {
                        Some(index)
                    } else {
                        None
                    }
                });
            if let Some(end) = pos {
                let end = index + 1 + end;
                iter.nth(end - index - 1);
                path.remove_in(index, end);
            }
            index += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::{InOrderCycleRemover, NodeId, Path, PathCycleRemover};

    #[test]
    fn path_simplify() {
        let mut path = Path::from([
            NodeId::from(1u128),
            NodeId::from(2u128),
            NodeId::from(3u128),
            NodeId::from(2u128),
            NodeId::from(5u128),
        ]);

        InOrderCycleRemover.remove_cycles_in_place(&mut path);

        assert_eq!(
            path,
            Path::from([
                NodeId::from(1u128),
                NodeId::from(2u128),
                NodeId::from(5u128),
            ])
        );
    }

    #[test]
    fn path_simplify_start() {
        let mut path = Path::from([
            NodeId::from(1u128),
            NodeId::from(2u128),
            NodeId::from(1u128),
            NodeId::from(4u128),
            NodeId::from(5u128),
        ]);

        InOrderCycleRemover.remove_cycles_in_place(&mut path);

        assert_eq!(
            path,
            Path::from([
                NodeId::from(1u128),
                NodeId::from(4u128),
                NodeId::from(5u128),
            ])
        );
    }

    #[test]
    fn path_simplify_end() {
        let mut path = Path::from([
            NodeId::from(1u128),
            NodeId::from(2u128),
            NodeId::from(3u128),
            NodeId::from(4u128),
            NodeId::from(3u128),
        ]);

        InOrderCycleRemover.remove_cycles_in_place(&mut path);

        assert_eq!(
            path,
            Path::from([
                NodeId::from(1u128),
                NodeId::from(2u128),
                NodeId::from(3u128),
            ])
        );
    }

    #[test]
    fn path_simplify_multiple() {
        let mut path = Path::from([
            NodeId::from(1u128),
            NodeId::from(2u128),
            NodeId::from(1u128),
            NodeId::from(3u128),
            NodeId::from(4u128),
            NodeId::from(3u128),
        ]);

        InOrderCycleRemover.remove_cycles_in_place(&mut path);

        assert_eq!(
            path,
            Path::from([
                NodeId::from(1u128),
                NodeId::from(3u128),
            ])
        );
    }
}
