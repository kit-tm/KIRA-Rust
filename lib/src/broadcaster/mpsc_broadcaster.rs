use std::sync::mpsc;
use std::sync::mpsc::{Receiver, SendError, SyncSender};

use crate::broadcaster::Broadcaster;
use crate::use_cases::UseCaseEvent;

/// A broadcaster using [std::sync::mpsc].
///
/// Therefore this implementation doesn't allow subscribing to the broadcaster.
/// Only on creation a single subscriber (Receiver) will be created.
///
/// This is useful in execution environments where multiple threads produce but only one thread
/// consumes the [UseCaseEvent]s.
#[derive(Debug, Clone)]
pub struct MPSCBroadcaster(SyncSender<UseCaseEvent>);

impl MPSCBroadcaster {
    pub fn new(size: usize) -> (Self, Receiver<UseCaseEvent>) {
        let (sender, receiver) = mpsc::sync_channel(size);
        (Self(sender), receiver)
    }
}

impl Broadcaster for MPSCBroadcaster {
    type SendError = SendError<UseCaseEvent>;
    type Subscriber = ();

    fn send_event(&self, event: UseCaseEvent) -> Result<(), Self::SendError> {
        self.0.send(event)
    }

    fn subscribe(&self) -> Self::Subscriber {}
}
