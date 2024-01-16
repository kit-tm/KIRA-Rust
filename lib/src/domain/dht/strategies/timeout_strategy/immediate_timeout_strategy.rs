use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use crate::domain::dht::strategies::timeout_strategy::TimeoutStrategy;

#[derive(Default, Debug, Clone)]
pub struct TaggedValue<V> {
    value: V,
    tagged: bool,
}

impl<V> TaggedValue<V> {
    pub fn new(value: V) -> Self {
        Self {
            value,
            tagged: false
        }
    }

    pub fn tag(mut self) -> Self {
        self.tagged = true;
        self
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

#[derive(Default)]
pub struct TaggedTimeoutStrategy<C, D> {
    _c: PhantomData<C>,
    _d: PhantomData<D>,
}

impl<C, D> TimeoutStrategy for TaggedTimeoutStrategy<C, TaggedValue<D>> {
    type Context = C;
    type Expirable = TaggedValue<D>;

    fn has_timed_out(&self, context: &Self::Context, expirable: &Self::Expirable) -> bool {
        expirable.tagged
    }
}