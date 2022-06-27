use std::sync::{Arc, Mutex};

use crate::broadcaster::Broadcaster;
use crate::use_cases::UseCaseEvent;

#[derive(Clone)]
pub struct BusBroadcaster(Arc<Mutex<bus::Bus<UseCaseEvent>>>);

impl BusBroadcaster {
    pub fn new(size: usize) -> Self {
        Self(Arc::new(Mutex::new(bus::Bus::new(size))))
    }
}

impl Broadcaster for BusBroadcaster {
    type SendError = ();
    type Subscriber = bus::BusReader<UseCaseEvent>;

    fn send_event(&self, event: UseCaseEvent) -> Result<(), Self::SendError> {
        self.0
            .lock()
            .expect("failed to get lock on bus")
            .broadcast(event);
        Ok(())
    }

    fn subscribe(&self) -> Self::Subscriber {
        self.0.lock().expect("failed to get lock in bus").add_rx()
    }
}
