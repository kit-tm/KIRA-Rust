use crate::broadcaster::Broadcaster;
use crate::usecases::UseCaseEvent;

impl<const ID_SIZE: usize> Broadcaster<ID_SIZE>
    for tokio::sync::broadcast::Sender<UseCaseEvent<ID_SIZE>>
{
    type SendError = tokio::sync::broadcast::error::SendError<UseCaseEvent<ID_SIZE>>;
    type Subscriber = tokio::sync::broadcast::Receiver<UseCaseEvent<ID_SIZE>>;

    fn send_event(&self, event: UseCaseEvent<ID_SIZE>) -> Result<(), Self::SendError> {
        self.send(event).map(|_| ())
    }

    fn subscribe(&self) -> Self::Subscriber {
        self.subscribe()
    }
}
