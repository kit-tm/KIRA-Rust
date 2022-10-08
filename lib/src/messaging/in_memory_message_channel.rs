use std::error::Error;
use std::fmt::Display;
use std::time::Duration;

use tokio::sync::broadcast::{self, Receiver, Sender};

use crate::domain::NetworkInterface;
use crate::messaging::error::SenderError;
use crate::messaging::messages::ProtocolMessage;
use crate::messaging::receiver::{RecvError, TryRecvError};
use crate::messaging::{AsyncProtocolMessageReceiver, ProtocolMessageSender};

const DEFAULT_CAPACITY: usize = 100;

/// A [MessageSender] and [MessageReceiver] which stores messages in a FIFO way.
///
/// Instead of waiting for incoming messages this implementation returns an error
/// if receive is called and the messages are empty.
#[derive(Debug)]
pub struct InMemoryMessageChannel {
    channel: (Sender<ProtocolMessage>, Receiver<ProtocolMessage>),
    interface: NetworkInterface,
}

impl Default for InMemoryMessageChannel {
    fn default() -> Self {
        Self {
            channel: broadcast::channel(DEFAULT_CAPACITY),
            interface: InMemoryMessageChannel::dummy_interface(),
        }
    }
}

impl InMemoryMessageChannel {
    pub fn new(capacity: usize, interface: NetworkInterface) -> Self {
        Self {
            channel: broadcast::channel(capacity),
            interface,
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            channel: broadcast::channel(capacity),
            interface: InMemoryMessageChannel::dummy_interface(),
        }
    }

    pub fn with_interface(interface: NetworkInterface) -> Self {
        Self {
            channel: broadcast::channel(DEFAULT_CAPACITY),
            interface,
        }
    }

    pub fn dummy_interface() -> NetworkInterface {
        NetworkInterface::new("dummy interface")
    }

    pub fn into_parts(self) -> (InMemorySender, InMemoryReceiver) {
        let (sender, receiver) = self.channel;
        (
            InMemorySender(sender.clone(), self.interface.clone()),
            InMemoryReceiver(sender, receiver, self.interface),
        )
    }
}

#[derive(Debug)]
pub struct InMemoryReceiver(
    Sender<ProtocolMessage>,
    Receiver<ProtocolMessage>,
    NetworkInterface,
);

impl Clone for InMemoryReceiver {
    fn clone(&self) -> Self {
        Self(self.0.clone(), self.0.subscribe(), self.2.clone())
    }
}

#[async_trait::async_trait]
impl AsyncProtocolMessageReceiver for InMemoryReceiver {
    async fn recv_timeout(
        &mut self,
        timeout: Option<Duration>,
    ) -> Result<Option<(ProtocolMessage, NetworkInterface)>, RecvError> {
        if let Some(duration) = timeout {
            match tokio::time::timeout(duration, self.1.recv()).await {
                Ok(Ok(message)) => Ok(Some((message, self.2.clone()))),
                Ok(Err(broadcast::error::RecvError::Closed)) => Err(RecvError::Closed),
                Ok(Err(e)) => Err(RecvError::Other(Box::new(e))),
                Err(_) => Err(RecvError::Timeout),
            }
        } else {
            match self.1.recv().await {
                Ok(message) => Ok(Some((message, self.2.clone()))),
                Err(broadcast::error::RecvError::Closed) => Err(RecvError::Closed),
                Err(e) => Err(RecvError::Other(Box::new(e))),
            }
        }
    }

    async fn try_recv(
        &mut self,
    ) -> Result<Option<(ProtocolMessage, NetworkInterface)>, TryRecvError> {
        match self.1.try_recv() {
            Ok(message) => Ok(Some((message, self.2.clone()))),
            Err(broadcast::error::TryRecvError::Empty) => Ok(None),
            Err(broadcast::error::TryRecvError::Closed) => Err(TryRecvError::Closed),
            Err(e) => Err(TryRecvError::Other(Box::new(e))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct InMemorySender(Sender<ProtocolMessage>, NetworkInterface);

#[derive(Debug, PartialEq, Eq)]
pub struct NoSendError;

impl Display for NoSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "This Message will never be seen!")
    }
}

impl Error for NoSendError {}

impl ProtocolMessageSender for InMemorySender {
    fn send_message<M>(&mut self, message: M) -> Result<(), SenderError>
    where
        M: Into<ProtocolMessage>,
    {
        let message = message.into();
        log::trace!("Sending message [to {}]: {:?}", self.1, message);
        if self.0.send(message).is_err() {
            return Err(SenderError::Closed);
        }
        Ok(())
    }
}

#[cfg(test)]
pub mod tests {
    use crate::domain::{NodeId, StateSeqNr};
    use crate::messaging::in_memory_message_channel::InMemoryMessageChannel;
    use crate::messaging::messages::{HelloMessage, ProtocolMessage};
    use crate::messaging::{AsyncProtocolMessageReceiver, ProtocolMessageSender};

    #[tokio::test]
    async fn dummy_message_hub_smoke_test() {
        let (mut sender, mut receiver) = InMemoryMessageChannel::with_capacity(2).into_parts();

        sender
            .send_message(HelloMessage {
                source: NodeId::zero(),
                source_state_seq_nr: StateSeqNr::from(0),
            })
            .unwrap();

        sender
            .send_message(ProtocolMessage::Hello(HelloMessage {
                source: NodeId::one(),
                source_state_seq_nr: StateSeqNr::from(0),
            }))
            .unwrap();

        let received = receiver.recv().await;
        assert!(received.is_ok(), "received error: {:?}", received);
        let received = received.unwrap();
        assert_eq!(
            received,
            Some((
                ProtocolMessage::Hello(HelloMessage {
                    source: NodeId::zero(),
                    source_state_seq_nr: StateSeqNr::from(0),
                }),
                InMemoryMessageChannel::dummy_interface()
            ))
        );

        let received = receiver.recv().await;
        assert!(received.is_ok(), "received error: {:?}", received);
        let received = received.unwrap();
        assert_eq!(
            received,
            Some((
                ProtocolMessage::Hello(HelloMessage {
                    source: NodeId::one(),
                    source_state_seq_nr: StateSeqNr::from(0),
                }),
                InMemoryMessageChannel::dummy_interface()
            ))
        );
    }
}
