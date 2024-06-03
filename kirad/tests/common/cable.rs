use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use tokio::sync::Mutex;

use kira_lib::domain::{NetworkInterface, NodeId};
use kira_lib::messaging::error::SenderError;
use kira_lib::messaging::{
    AsyncProtocolMessageSender, InMemoryMessageChannel, InMemoryReceiver, InMemorySender,
    ProtocolMessage, ProtocolMessageSender,
};

/// Link which connects two nodes.
#[derive(Debug)]
pub struct Cable {
    one: NodeId,
    two: NodeId,
    // Cables are used to distinguish between the communication direction
    // Node one has cable_one as receiver and cable_two as sender
    endpoint_one: (CloseableSender, Option<InMemoryReceiver>),
    endpoint_two: (CloseableSender, Option<InMemoryReceiver>),
}

impl Cable {
    pub fn new(one: NodeId, two: NodeId) -> Self {
        let interface_one = NetworkInterface::dummy(format!("{}--{}", one, two));
        let (cable_one_sender, cable_one_receiver) =
            InMemoryMessageChannel::with_interface(interface_one.clone()).into_parts();
        let (cable_two_sender, cable_two_receiver) =
            InMemoryMessageChannel::with_interface(interface_one).into_parts();
        Self {
            one,
            two,
            endpoint_one: (cable_one_sender.into(), Some(cable_two_receiver)),
            endpoint_two: (cable_two_sender.into(), Some(cable_one_receiver)),
        }
    }

    fn take_endpoint(
        (sender, receiver): &mut (CloseableSender, Option<InMemoryReceiver>),
    ) -> Option<(CloseableSender, InMemoryReceiver)> {
        receiver.take().map(|receiver| (sender.clone(), receiver))
    }

    pub fn take_parts_for(&mut self, id: &NodeId) -> Option<(CloseableSender, InMemoryReceiver)> {
        match (id == &self.one, id == &self.two) {
            (true, _) => Cable::take_endpoint(&mut self.endpoint_one),
            (_, true) => Cable::take_endpoint(&mut self.endpoint_two),
            _ => None,
        }
    }

    fn interface(&self) -> NetworkInterface {
        NetworkInterface::dummy(format!("{}--{}", self.one, self.two))
    }

    pub fn blocking_close(&self) {
        // To close the senders have to be dropped
        log::trace!("Closed channels {:?}", self.interface());
        self.endpoint_one.0.blocking_close();
        self.endpoint_two.0.blocking_close();
    }

    pub async fn close(&self) {
        log::trace!("Closed channels {:?}", self.interface());
        self.endpoint_one.0.close().await;
        self.endpoint_two.0.close().await;
    }
}

#[derive(Debug, Clone)]
pub struct CloseableSender {
    sender: Arc<Mutex<Option<InMemorySender>>>,
}

impl From<InMemorySender> for CloseableSender {
    fn from(inner: InMemorySender) -> Self {
        Self {
            sender: Arc::new(Mutex::new(Some(inner))),
        }
    }
}

impl CloseableSender {
    pub fn blocking_close(&self) {
        self.sender.blocking_lock().take();
    }

    pub async fn close(&self) {
        self.sender.lock().await.take();
    }

    pub fn interface(&self) -> Option<NetworkInterface> {
        self.sender
            .blocking_lock()
            .deref()
            .as_ref()
            .map(|sender| sender.interface().clone())
    }
}

impl ProtocolMessageSender for CloseableSender {
    fn send_message<M>(&mut self, message: M) -> Result<(), SenderError>
    where
        M: Into<ProtocolMessage>,
    {
        match self.sender.blocking_lock().deref_mut() {
            Some(sender) => sender.send_message(message),
            None => Err(SenderError::Closed),
        }
    }
}

#[async_trait::async_trait]
impl AsyncProtocolMessageSender for CloseableSender {
    async fn send_message<M>(&mut self, message: M) -> Result<(), SenderError>
    where
        M: Into<ProtocolMessage> + Send,
    {
        match self.sender.lock().await.deref_mut() {
            Some(sender) => sender.send_message(message),
            None => Err(SenderError::Closed),
        }
    }
}
