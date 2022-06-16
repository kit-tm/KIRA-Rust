use std::{time::Duration, ops::Deref};

pub struct TimeoutId(usize);

impl From<usize> for TimeoutId {
    fn from(raw: usize) -> Self {
        Self(raw)
    }
}

impl Deref for TimeoutId {
    type Target = usize;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub type Callback = fn(TimeoutId);

pub trait Runtime {
    fn start_timeout(&mut self, duration: Duration, callback: Callback) -> TimeoutId;
    fn delete_timeout(&mut self, id: TimeoutId);
}

pub struct StdRuntime {
    
}

impl Runtime for StdRuntime {
    fn start_timeout(&mut self, duration: Duration, callback: Callback) -> TimeoutId {
        todo!()
    }

    fn delete_timeout(&mut self, id: TimeoutId) {
        todo!()
    }
}