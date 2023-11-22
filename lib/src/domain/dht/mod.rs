use std::time::Instant;

pub mod hash_table;
pub mod expiring_hash_table;

pub trait TimeoutStrategy<C> {
    fn is_timed_out(&self, context: &C, time: &Instant) -> bool;
}

pub trait Expiring<C> {
    fn collect(&mut self, strategy: &impl TimeoutStrategy<C>) -> bool;
}

pub trait ContextExpiring<C> {
    fn collect_with_context(&mut self, context: &C, strategy: &impl TimeoutStrategy<C>) -> bool;
}
