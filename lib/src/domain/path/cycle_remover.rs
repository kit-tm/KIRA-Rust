use crate::domain::Path;

/// An algorithm to remove cycles from a [Path].
pub trait PathCycleRemover {
    fn remove_cycles_in_place(&mut self, path: &mut Path);

    fn with_cycles_removed(&mut self, path: &Path) -> Path {
        let mut path = path.clone();
        self.remove_cycles_in_place(&mut path);
        path
    }
}
