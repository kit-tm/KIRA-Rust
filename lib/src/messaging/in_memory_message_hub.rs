use std::collections::VecDeque;
use std::error::Error;
use std::fmt::Display;
use std::time::Duration;

use crate::domain::Port;
use crate::messaging::messages::Message;
use crate::messaging::receiver::{MessageReceiver, RecvTimeout, TryRecvError};
use crate::messaging::sender::MessageSender;

/// A [MessageSender] and [MessageReceiver] which stores messages in a FIFO way.
///
/// Instead of waiting for incoming messages this implementation returns an error
/// if receive is called and the messages are empty.
#[derive(Debug)]
pub struct InMemoryMessageHub {
    messages: VecDeque<Message>,
}

impl Default for InMemoryMessageHub {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryMessageHub {
    pub fn new() -> Self {
        Self {
            messages: VecDeque::new(),
        }
    }

    pub fn dummy_port() -> Port {
        Port::new(String::from("dummy port"))
    }

    fn pop(&mut self) -> Option<(Message, Port)> {
        self.messages
            .pop_front()
            .map(|message| (message, Self::dummy_port()))
    }
}

impl MessageReceiver for InMemoryMessageHub {
    fn recv_timeout(
        &mut self,
        _timeout: Option<Duration>,
    ) -> Result<Option<(Message, Port)>, RecvTimeout> {
        Ok(self.pop())
    }

    fn try_recv(&mut self) -> Result<Option<(Message, Port)>, TryRecvError> {
        Ok(self.pop())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct NoSendError;

impl Display for NoSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "This Message will never be seen!")
    }
}

impl Error for NoSendError {}

impl MessageSender for InMemoryMessageHub {
    // Must not return Errors.
    type Error = NoSendError;

    fn send<M>(&mut self, message: M) -> Result<(), Self::Error>
    where
        M: Into<Message>,
    {
        self.messages.push_back(message.into());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::NodeId;
    use crate::messaging::in_memory_message_hub::InMemoryMessageHub;
    use crate::messaging::messages::{HelloMessage, Message};
    use crate::messaging::receiver::MessageReceiver;
    use crate::messaging::sender::MessageSender;

    #[test]
    fn dummy_message_hub_smoke_test() {
        let mut hub = InMemoryMessageHub::new();

        hub.send(Message::Hello(HelloMessage {
            source: NodeId::zero(),
            destination: NodeId::zero(),
        }))
        .unwrap();

        hub.send(Message::Hello(HelloMessage {
            source: NodeId::one(),
            destination: NodeId::one(),
        }))
        .unwrap();

        assert_eq!(
            hub.recv(),
            Some((
                Message::Hello(HelloMessage {
                    source: NodeId::zero(),
                    destination: NodeId::zero(),
                }),
                InMemoryMessageHub::dummy_port()
            ))
        );

        assert_eq!(
            hub.recv(),
            Some((
                Message::Hello(HelloMessage {
                    source: NodeId::one(),
                    destination: NodeId::one(),
                }),
                InMemoryMessageHub::dummy_port()
            ))
        );
    }
}
