use crate::broadcaster::Broadcaster;
use crate::use_cases::UseCaseEvent;

impl Broadcaster for tokio::sync::broadcast::Sender<UseCaseEvent> {
    type SendError = tokio::sync::broadcast::error::SendError<UseCaseEvent>;
    type Subscriber = tokio::sync::broadcast::Receiver<UseCaseEvent>;

    fn send_event(&self, event: UseCaseEvent) -> Result<(), Self::SendError> {
        self.send(event).map(|_| ())
    }

    fn subscribe(&self) -> Self::Subscriber {
        self.subscribe()
    }
}

impl Broadcaster for tokio::sync::mpsc::UnboundedSender<UseCaseEvent> {
    type SendError = tokio::sync::mpsc::error::SendError<UseCaseEvent>;
    // Creating a new subscriber is not permitted as mpsc channels only have one consumer.
    type Subscriber = ();

    fn send_event(&self, event: UseCaseEvent) -> Result<(), Self::SendError> {
        self.send(event)
    }

    fn subscribe(&self) -> Self::Subscriber {}
}
