use crate::broadcaster::Broadcaster;
use crate::runtime::Runtime;
use crate::usecases::{TimerId, UseCaseEvent};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

pub struct TokioRuntime<B: Broadcaster> {
    counter: Mutex<usize>,
    executioner: B,
    runtime: Arc<tokio::runtime::Runtime>,
}

impl<B: Broadcaster> TokioRuntime<B> {
    pub fn new(executioner: B, runtime: Arc<tokio::runtime::Runtime>) -> Self {
        Self {
            counter: Mutex::new(0),
            executioner,
            runtime,
        }
    }

    pub fn runtime(&self) -> &tokio::runtime::Runtime {
        &self.runtime
    }
}

impl<B: 'static + Broadcaster + Send + Sync> Runtime for TokioRuntime<B> {
    fn register_timer(&self, duration: Duration) -> TimerId {
        let timer_id = {
            let mut lock = self.runtime.block_on(self.counter.lock());
            let id = *lock;
            *lock = id + 1;
            TimerId::from(id)
        };

        let broadcaster = self.executioner.clone();
        self.runtime.spawn(async move {
            tokio::time::sleep(duration).await;
            if let Err(e) = broadcaster.send_event(UseCaseEvent::Timer(timer_id)) {
                log::error!("Failed to send Event to use cases: {}", e);
            }
        });

        timer_id
    }
}
