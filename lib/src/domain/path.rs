use crate::domain::Id;

/// A Path of [Id]s.
///
/// As Paths don't have a fixed length, a [Vec] has to be used here.
pub struct Path<T: Id>(Vec<T>);
