use crate::domain::Path;

use derive_more::derive::Display;
use std::hash::{Hash, Hasher};

const MAX_ALTERNATIVE_PATHS: usize = 3;

#[derive(Default, Debug, Clone, Eq, Display)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[display("PathColl [active: {:?}, proposed: {:?}], alternatives: {:?}", self.active_path, self.proposed_path, self.alternative_paths)]
pub struct PathCollection {
    active_path: Option<Path>,
    #[cfg_attr(feature = "serde", serde(skip))]
    proposed_path: Option<Path>,

    #[cfg_attr(feature = "serde", serde(skip))]
    alternative_paths: [Option<Path>; MAX_ALTERNATIVE_PATHS],
}

impl PathCollection {
    pub fn new() -> Self {
        Self {
            ..Default::default()
        }
    }

    pub fn new_with_active_path(active_path: Path) -> Self {
        Self {
            active_path: Some(active_path),
            ..Default::default()
        }
    }

    pub fn set_active_path(&mut self, new_active_path: Path) {
        self.active_path = Some(new_active_path);
    }

    pub fn active_path(&self) -> Option<&Path> {
        self.active_path.as_ref()
    }

    pub fn into_active_path(self) -> Option<Path> {
        self.active_path
    }

    pub fn active_path_mut(&mut self) -> Option<&mut Path> {
        self.active_path.as_mut()
    }

    pub fn set_proposed_path(&mut self, new: Path) {
        self.proposed_path = Some(new);
    }

    pub fn proposed_path(&self) -> Option<&Path> {
        self.proposed_path.as_ref()
    }

    pub fn set_proposed_to_active(&mut self) {
        self.active_path = self.proposed_path.take();
    }

    pub fn first_alternative_path(&self) -> Option<&Path> {
        self.alternative_paths
            .iter()
            .find(|x| x.is_some())?
            .as_ref()
    }

    // TODO this should also follow a smarter strategy, e.g., replacing longer and older paths
    pub fn move_active_to_alternative(&mut self) {
        if let Some(replaceable) = self.alternative_paths.iter().position(|x| x.is_none()) {
            self.alternative_paths[replaceable] = self.active_path.take();
        } else {
            let replace_it = self
                .alternative_paths
                .last_mut()
                .expect("alternative_paths has at least one element");
            *replace_it = self.active_path.take();
        }
    }
}

impl PartialEq for PathCollection {
    fn eq(&self, other: &Self) -> bool {
        self.active_path == other.active_path && self.proposed_path == other.proposed_path
    }
}

// NOTE currently the hasher only considers the active path if there is one
impl Hash for PathCollection {
    fn hash<H: Hasher>(&self, state: &mut H) {
        if let Some(ref apath) = self.active_path {
            Hash::hash(apath, state)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::NodeId;

    static NODE_ID_ONE: NodeId = NodeId::from_const(1u128);
    static NODE_ID_TWO: NodeId = NodeId::from_const(2u128);
    static NODE_ID_THREE: NodeId = NodeId::from_const(3u128);
    static NODE_ID_FOUR: NodeId = NodeId::from_const(4u128);
    static NODE_ID_FIVE: NodeId = NodeId::from_const(5u128);

    #[test]
    fn test_new() {
        let pc = PathCollection::new();
        assert!(pc.active_path.is_none());
        assert!(pc.proposed_path.is_none());
        assert!(pc.alternative_paths.iter().all(|x| x.is_none()));
    }

    #[test]
    fn test_new_with_path() {
        let p = Path::try_from(vec![
            NODE_ID_ONE,
            NODE_ID_TWO,
            NODE_ID_THREE,
            NODE_ID_FOUR,
            NODE_ID_FIVE,
        ])
        .expect("not empty");

        let pc = PathCollection::new_with_active_path(p);
        if let Some(stored_path) = pc.active_path() {
            assert_eq!(NODE_ID_ONE, stored_path.ids[0]);
            assert_eq!(NODE_ID_TWO, stored_path.ids[1]);
            assert_eq!(NODE_ID_THREE, stored_path.ids[2]);
            assert_eq!(NODE_ID_FOUR, stored_path.ids[3]);
            assert_eq!(NODE_ID_FIVE, stored_path.ids[4]);
        } else {
            panic!("Should not be none!");
        }
    }

    #[test]
    fn test_set_proposed_path() {
        let p = Path::try_from(vec![
            NODE_ID_ONE,
            NODE_ID_TWO,
            NODE_ID_THREE,
            NODE_ID_FOUR,
            NODE_ID_FIVE,
        ])
        .expect("not empty");
        let mut pc = PathCollection::new();
        pc.set_proposed_path(p.clone());
        if let Some(r) = pc.proposed_path() {
            assert!(*r == p);
        } else {
            panic!("proposed path assumed to be not None");
        }
    }

    #[test]
    fn test_set_proposed_to_active() {
        let p = Path::try_from(vec![
            NODE_ID_ONE,
            NODE_ID_TWO,
            NODE_ID_THREE,
            NODE_ID_FOUR,
            NODE_ID_FIVE,
        ])
        .expect("not empty");
        let mut pc = PathCollection::new();
        pc.set_proposed_path(p.clone());
        pc.set_proposed_to_active();
        if let Some(r) = pc.active_path() {
            assert!(*r == p);
            assert!(pc.proposed_path.is_none());
        } else {
            panic!("active path assumed to be not None");
        }
    }

    #[test]
    fn move_proposed_to_active_to_alternative() {
        let p = Path::try_from(vec![
            NODE_ID_ONE,
            NODE_ID_TWO,
            NODE_ID_THREE,
            NODE_ID_FOUR,
            NODE_ID_FIVE,
        ])
        .expect("not empty");
        let mut pc = PathCollection::new();
        pc.set_proposed_path(p.clone());
        pc.set_proposed_to_active();
        assert!(pc.proposed_path.is_none());
        pc.move_active_to_alternative();
        assert!(pc.active_path().is_none());
        if let Some(r) = pc.first_alternative_path() {
            assert!(*r == p);
        } else {
            panic!("at least one alternative path must be present");
        }
        let q = Path::try_from(vec![NODE_ID_THREE, NODE_ID_FOUR, NODE_ID_FIVE]).expect("not empty");
        let r = Path::try_from(vec![NODE_ID_ONE, NODE_ID_FOUR, NODE_ID_TWO]).expect("not empty");
        let s = Path::try_from(vec![NODE_ID_FIVE, NODE_ID_THREE, NODE_ID_ONE]).expect("not empty");
        pc.set_active_path(q.clone());
        pc.move_active_to_alternative();
        pc.set_active_path(r.clone());
        pc.move_active_to_alternative();
        let mut alt_paths_it = pc.alternative_paths.iter();
        assert_eq!(
            *alt_paths_it
                .next()
                .expect("alternative path 1 should be present"),
            Some(p)
        );
        assert_eq!(
            *alt_paths_it
                .next()
                .expect("alternative path 2 should be present"),
            Some(q)
        );
        assert_eq!(
            *alt_paths_it
                .next()
                .expect("alternative path 3 should be present"),
            Some(r)
        );
        assert_eq!(alt_paths_it.next(), None);
        pc.set_active_path(s.clone());
        pc.move_active_to_alternative();
        assert_eq!(
            *pc.alternative_paths
                .last()
                .expect("last alternative path should not be None"),
            Some(s)
        );
    }
}
