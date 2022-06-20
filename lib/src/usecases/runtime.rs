use std::ops::Deref;
use std::time::Duration;

use crate::usecases::executioner::Executioner;
use crate::usecases::UseCaseEvent;

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct TimerId(usize);

impl From<usize> for TimerId {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

impl Deref for TimerId {
    type Target = usize;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub trait Runtime {
    /// Either waits the duration instantly or returns and
    fn wait(&mut self, duration: Duration);
}

/// Single Threaded Runtime using the standard library.
pub struct StdSyncRuntime<E, const ID_SIZE: usize> {
    id_counter: usize,
    executioner: E,
}

impl<E: Default, const ID_SIZE: usize> Default for StdSyncRuntime<E, ID_SIZE> {
    fn default() -> Self {
        Self {
            id_counter: 0,
            executioner: E::default(),
        }
    }
}

impl<E, const ID_SIZE: usize> StdSyncRuntime<E, ID_SIZE> {
    pub const fn new(executioner: E) -> Self {
        StdSyncRuntime {
            id_counter: 0,
            executioner,
        }
    }
}

impl<E: Executioner<ID_SIZE>, const ID_SIZE: usize> Runtime for StdSyncRuntime<E, ID_SIZE> {
    fn wait(&mut self, duration: Duration) {
        std::thread::sleep(duration);
        let id = TimerId(self.id_counter);
        self.id_counter += 1;
        self.executioner.send_event(UseCaseEvent::Timer(id));
    }
}
