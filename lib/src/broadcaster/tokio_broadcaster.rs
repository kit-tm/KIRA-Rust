use crate::broadcaster::Broadcaster;
use crate::usecases::UseCaseEvent;

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
