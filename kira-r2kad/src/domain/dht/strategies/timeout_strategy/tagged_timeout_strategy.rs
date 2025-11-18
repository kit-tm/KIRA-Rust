use crate::domain::dht::strategies::timeout_strategy::TimeoutStrategy;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

/// A value which contains a [bool] tag.
///
/// This struct is used together with the [TaggedTimeoutStrategy].
#[derive(Default, Debug, Clone)]
pub struct TaggedValue<V> {
    value: V,
    tagged: bool,
}

#[allow(dead_code)]
impl<V> TaggedValue<V> {
    /// Creates a new instance of `Self` with the specified value.
    ///
    /// # Arguments
    ///
    /// * `value` - The value to be stored in the instance.
    ///
    /// # Returns
    ///
    /// A new instance of `Self` with the specified value.
    pub fn new(value: V) -> Self {
        Self {
            value,
            tagged: false,
        }
    }

    /// Sets the `tagged` flag to `true` of itself.
    ///
    /// # Returns
    ///
    /// Returns itself with the tagged flag set to `true`.
    pub fn tag(mut self) -> Self {
        self.tagged = true;
        self
    }

    /// Returns a boolean value indicating whether this instance is tagged or not.
    ///
    /// # Returns
    ///
    /// - `true` if the instance is tagged.
    /// - `false` if the instance is not tagged.
    pub fn tagged(&self) -> bool {
        self.tagged
    }
}

impl<V: PartialEq> PartialEq<Self> for TaggedValue<V> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<V: Eq> Eq for TaggedValue<V> {}

impl<V: Hash> Hash for TaggedValue<V> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.hash(state)
    }
}

/// A [TimeoutStrategy] that is used for testing only.
///
/// This strategy decides if [TaggedValue]s are expired.
/// [TaggedValue]s are considered expired if they are tagged.
///
/// The [TimeoutStrategy::Context] is arbitrary and not used.
#[derive(Default)]
pub struct TaggedTimeoutStrategy<C, D> {
    _c: PhantomData<C>,
    _d: PhantomData<D>,
}

impl<C, D> TimeoutStrategy for TaggedTimeoutStrategy<C, TaggedValue<D>> {
    type Context = C;
    type Expirable = TaggedValue<D>;

    fn has_timed_out(&self, _context: &Self::Context, expirable: &Self::Expirable) -> bool {
        expirable.tagged()
    }
}
