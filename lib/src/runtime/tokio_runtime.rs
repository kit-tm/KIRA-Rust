use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use crate::broadcaster::Broadcaster;
use crate::runtime::Runtime;
use crate::use_cases::{TimerId, UseCaseEvent};

pub struct TokioRuntime<B: Broadcaster> {
    // Used to generate unique ids and also keep track of used ids in case of overflow
    counter_and_used_counters: Arc<Mutex<(usize, HashSet<usize>)>>,
    executioner: B,
    runtime: Arc<tokio::runtime::Runtime>,
}

impl<B: Broadcaster> TokioRuntime<B> {
    pub fn new(executioner: B, runtime: Arc<tokio::runtime::Runtime>) -> Self {
        Self {
            counter_and_used_counters: Arc::new(Mutex::new((0, HashSet::new()))),
            executioner,
            runtime,
        }
    }

    pub fn runtime(&self) -> &tokio::runtime::Runtime {
        &self.runtime
    }

    // Handles overflow by using a set of used counters
    fn create_new_id(&self) -> TimerId {
        let mut lock = self.runtime.block_on(self.counter_and_used_counters.lock());
        // Increase id until not used id is found
        let mut id = lock.0;
        while lock.1.contains(&id) {
            id += 1
        }
        // Set the current counter and add new id to used ones
        lock.0 = id;
        lock.1.insert(id);

        TimerId::from(id)
    }

    async fn wait_and_send_timer_event(
        broadcaster: &B,
        duration: Duration,
        timer_id: TimerId,
    ) -> Result<(), TaskError> {
        tokio::time::sleep(duration).await;
        if let Err(e) = broadcaster.send_event(UseCaseEvent::Timer(timer_id)) {
            log::error!("Failed to send Event to use cases: {}", e);
            return Err(TaskError::BroadcastFailed);
        }

        Ok(())
    }
}

impl<B: 'static + Broadcaster + Send + Sync> Runtime for TokioRuntime<B> {
    fn register_timer(&self, duration: Duration) -> TimerId {
        let timer_id = self.create_new_id();

        let use_case_broadcaster = self.executioner.clone();
        let counters = Arc::clone(&self.counter_and_used_counters);
        self.runtime.spawn(async move {
            // Ignored as one-shot timer result is irrelevant
            let _ =
                TokioRuntime::wait_and_send_timer_event(&use_case_broadcaster, duration, timer_id)
                    .await;
            remove_used_id(counters, &timer_id).await;
        });

        timer_id
    }

    fn register_periodic_timer(&self, duration: Duration) -> TimerId {
        let timer_id = self.create_new_id();

        let use_case_broadcaster = self.executioner.clone();
        let counters = Arc::clone(&self.counter_and_used_counters);
        self.runtime.spawn(async move {
            // Repeat until error is returned
            while (TokioRuntime::wait_and_send_timer_event(
                &use_case_broadcaster,
                duration,
                timer_id,
            )
            .await)
                .is_ok()
            {}
            remove_used_id(counters, &timer_id).await;
        });

        timer_id
    }
}

enum TaskError {
    BroadcastFailed,
}

async fn remove_used_id(mutex: Arc<Mutex<(usize, HashSet<usize>)>>, id: &TimerId) {
    let mut lock = mutex.lock().await;
    lock.1.remove(id);
}
