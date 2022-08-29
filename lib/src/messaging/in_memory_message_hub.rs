use std::collections::VecDeque;
use std::error::Error;
use std::fmt::Display;
use std::time::Duration;

use crate::domain::Port;
use crate::messaging::error::SenderError;
use crate::messaging::messages::ProtocolMessage;
use crate::messaging::receiver::{ProtocolMessageReceiver, RecvError, TryRecvError};
use crate::messaging::sender::ProtocolMessageSender;

/// A [MessageSender] and [MessageReceiver] which stores messages in a FIFO way.
///
/// Instead of waiting for incoming messages this implementation returns an error
/// if receive is called and the messages are empty.
#[derive(Debug)]
pub struct InMemoryMessageHub {
    messages: VecDeque<ProtocolMessage>,
    port: Port,
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
            port: InMemoryMessageHub::dummy_port(),
        }
    }

    pub fn dummy_port() -> Port {
        Port::Named(String::from("dummy port"))
    }

    fn pop(&mut self) -> Option<(ProtocolMessage, Port)> {
        self.messages
            .pop_front()
            .map(|message| (message, self.port.clone()))
    }

    pub fn messages(&self) -> impl Iterator<Item = (&ProtocolMessage, &Port)> {
        self.messages.iter().map(|message| (message, &self.port))
    }
}

impl ProtocolMessageReceiver for InMemoryMessageHub {
    fn recv_timeout(
        &mut self,
        _timeout: Option<Duration>,
    ) -> Result<Option<(ProtocolMessage, Port)>, RecvError> {
        Ok(self.pop())
    }

    fn try_recv(&mut self) -> Result<Option<(ProtocolMessage, Port)>, TryRecvError> {
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

impl ProtocolMessageSender for InMemoryMessageHub {
    fn send<M>(&mut self, message: M) -> Result<(), SenderError>
    where
        M: Into<ProtocolMessage>,
    {
        let message = message.into();
        log::trace!("Sent message: {:?}", message);
        self.messages.push_back(message);
        Ok(())
    }
}

#[cfg(test)]
pub mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use crate::domain::{NodeId, Port, StateSeqNr};
    use crate::messaging::error::SenderError;
    use crate::messaging::in_memory_message_hub::InMemoryMessageHub;
    use crate::messaging::messages::{HelloMessage, ProtocolMessage};
    use crate::messaging::receiver::ProtocolMessageReceiver;
    use crate::messaging::sender::ProtocolMessageSender;
    use crate::messaging::{RecvError, TryRecvError};

    #[derive(Debug, Clone)]
    pub struct ArcSyncInMemoryMessageHub(Arc<Mutex<InMemoryMessageHub>>);

    impl Default for ArcSyncInMemoryMessageHub {
        fn default() -> Self {
            ArcSyncInMemoryMessageHub::new()
        }
    }

    impl ArcSyncInMemoryMessageHub {
        pub fn new() -> Self {
            Self(Arc::new(Mutex::new(InMemoryMessageHub::new())))
        }

        pub fn messages(&self) -> Vec<(ProtocolMessage, Port)> {
            self.0
                .lock()
                .expect("failed to get hub lock")
                .messages()
                .map(|(message, port)| (message.clone(), port.clone()))
                .collect()
        }
    }

    impl ProtocolMessageSender for ArcSyncInMemoryMessageHub {
        fn send<M>(&mut self, message: M) -> Result<(), SenderError>
        where
            M: Into<ProtocolMessage>,
        {
            let mut lock = self.0.lock().expect("failed to get lock on hub");
            lock.send(message)
        }
    }

    impl ProtocolMessageReceiver for ArcSyncInMemoryMessageHub {
        fn recv_timeout(
            &mut self,
            timeout: Option<Duration>,
        ) -> Result<Option<(ProtocolMessage, Port)>, RecvError> {
            let mut lock = self.0.lock().expect("failed to get lock on hub");
            lock.recv_timeout(timeout)
        }

        fn try_recv(&mut self) -> Result<Option<(ProtocolMessage, Port)>, TryRecvError> {
            let mut lock = self.0.lock().expect("failed to get lock on hub");
            lock.try_recv()
        }
    }

    #[test]
    fn dummy_message_hub_smoke_test() {
        let mut hub = InMemoryMessageHub::new();

        hub.send(ProtocolMessage::Hello(HelloMessage {
            source: NodeId::zero(),
            source_state_seq_nr: StateSeqNr::from(0),
        }))
        .unwrap();

        hub.send(ProtocolMessage::Hello(HelloMessage {
            source: NodeId::one(),
            source_state_seq_nr: StateSeqNr::from(0),
        }))
        .unwrap();

        assert_eq!(
            hub.recv(),
            Some((
                ProtocolMessage::Hello(HelloMessage {
                    source: NodeId::zero(),
                    source_state_seq_nr: StateSeqNr::from(0),
                }),
                InMemoryMessageHub::dummy_port()
            ))
        );

        assert_eq!(
            hub.recv(),
            Some((
                ProtocolMessage::Hello(HelloMessage {
                    source: NodeId::one(),
                    source_state_seq_nr: StateSeqNr::from(0),
                }),
                InMemoryMessageHub::dummy_port()
            ))
        );
    }
}
