use std::{collections::VecDeque, fmt::Display};

use crate::domain::{Contact, NodeId};

#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub struct Nonce(u128);

impl From<u128> for Nonce {
    fn from(value: u128) -> Self {
        Self(value)
    }
}

impl Nonce {
    /// Creates a random [Nonce].‚
    pub fn random() -> Self {
        Self(rand::random())
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Message<const ID_SIZE: usize> {
    Hello(HelloMessage<ID_SIZE>),
    PNDiscReq(ReqRspMessage<PNDiscReqData<ID_SIZE>, ID_SIZE>),
    PNDiscRsp(ReqRspMessage<PNDiscRspData<ID_SIZE>, ID_SIZE>),
    FindNodeReq(ReqRspMessage<FindNodeReqData, ID_SIZE>),
    FindNodeRsp(ReqRspMessage<FindNodeRspData<ID_SIZE>, ID_SIZE>),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct HelloMessage<const ID_SIZE: usize> {
    pub source: NodeId<ID_SIZE>,
    pub destination: NodeId<ID_SIZE>,
}

impl<const ID_SIZE: usize> From<HelloMessage<ID_SIZE>> for Message<ID_SIZE> {
    fn from(message: HelloMessage<ID_SIZE>) -> Self {
        Self::Hello(message)
    }
}

/// In contrary to a [HelloMessage] this type contains a [Nonce] to
/// identify Request and Response Pairs.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ReqRspMessage<T: std::fmt::Debug, const ID_SIZE: usize> {
    pub id: Nonce,
    pub source: NodeId<ID_SIZE>,
    pub destination: NodeId<ID_SIZE>,
    pub data: T,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct PNDiscReqData<const ID_SIZE: usize> {
    pub contacts: Vec<NodeId<ID_SIZE>>,
}

impl<const ID_SIZE: usize> From<ReqRspMessage<PNDiscReqData<ID_SIZE>, ID_SIZE>>
    for Message<ID_SIZE>
{
    fn from(message: ReqRspMessage<PNDiscReqData<ID_SIZE>, ID_SIZE>) -> Self {
        Self::PNDiscReq(message)
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct PNDiscRspData<const ID_SIZE: usize> {
    pub contacts: Vec<NodeId<ID_SIZE>>,
}

impl<const ID_SIZE: usize> From<ReqRspMessage<PNDiscRspData<ID_SIZE>, ID_SIZE>>
    for Message<ID_SIZE>
{
    fn from(message: ReqRspMessage<PNDiscRspData<ID_SIZE>, ID_SIZE>) -> Self {
        Self::PNDiscRsp(message)
    }
}

/// The target of the request is located at the destination id of
/// the [ReqRspMessage].
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct FindNodeReqData;

impl<const ID_SIZE: usize> From<ReqRspMessage<FindNodeReqData, ID_SIZE>> for Message<ID_SIZE> {
    fn from(message: ReqRspMessage<FindNodeReqData, ID_SIZE>) -> Self {
        Self::FindNodeReq(message)
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct FindNodeRspData<const ID_SIZE: usize> {
    pub contacts: Vec<Contact<ID_SIZE>>,
}

impl<const ID_SIZE: usize> From<ReqRspMessage<FindNodeRspData<ID_SIZE>, ID_SIZE>>
    for Message<ID_SIZE>
{
    fn from(message: ReqRspMessage<FindNodeRspData<ID_SIZE>, ID_SIZE>) -> Self {
        Self::FindNodeRsp(message)
    }
}

/// Sends [Message]s to other Nodes.
///
/// Derives how and where to send the [Message] by analyzing its fields
/// and converts them to an appropriate format so that the corresponding
/// [MessageReceiver] can convert it back to a [Message].
pub trait MessageSender<const ID_SIZE: usize> {
    type Error: std::error::Error;

    /// Sends a [Message] to another Node, converting it to an appropriate
    /// format before sending.
    ///
    /// Returns an Error if the operation or formatting failed.
    fn send<M>(&mut self, message: M) -> Result<(), Self::Error>
    where
        M: Into<Message<ID_SIZE>>;
}

/// Receives [Message]s of other Nodes.
///
/// Converts a [Message] formatted by its corresponding [MessageSender] back
/// to a [Message] and returns it.
pub trait MessageReceiver<const ID_SIZE: usize> {
    type Error;

    /// Receives a [Message] or an [Error] if receiving failed.
    fn receive(&mut self) -> Result<Message<ID_SIZE>, Self::Error>;
}

/// A [MessageSender] and [MessageReceiver] which stores messages in a FIFO way.
#[derive(Debug)]
pub struct DummyMessageHub<const ID_SIZE: usize> {
    messages: VecDeque<Message<ID_SIZE>>,
}

impl<const ID_SIZE: usize> Default for DummyMessageHub<ID_SIZE> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const ID_SIZE: usize> DummyMessageHub<ID_SIZE> {
    pub fn new() -> Self {
        Self {
            messages: VecDeque::new(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct NoMessagePendingError;

impl<const ID_SIZE: usize> MessageReceiver<ID_SIZE> for DummyMessageHub<ID_SIZE> {
    type Error = NoMessagePendingError;

    fn receive(&mut self) -> Result<Message<ID_SIZE>, Self::Error> {
        self.messages.pop_front().ok_or(NoMessagePendingError)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct NoSendError;

impl Display for NoSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "This Message will never be seen!")
    }
}

impl std::error::Error for NoSendError {}

impl<const ID_SIZE: usize> MessageSender<ID_SIZE> for DummyMessageHub<ID_SIZE> {
    // Must not return Errors.
    type Error = NoSendError;

    fn send<M>(&mut self, message: M) -> Result<(), Self::Error>
    where
        M: Into<Message<ID_SIZE>>,
    {
        self.messages.push_back(message.into());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        domain::NodeId,
        messaging::{MessageReceiver, MessageSender},
    };

    use super::{DummyMessageHub, HelloMessage, Message};

    #[test]
    fn dummy_message_hub_smoke_test() {
        let mut hub = DummyMessageHub::<1>::new();

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
            hub.receive(),
            Ok(Message::Hello(HelloMessage {
                source: NodeId::zero(),
                destination: NodeId::zero(),
            }))
        );
        assert_eq!(
            hub.receive(),
            Ok(Message::Hello(HelloMessage {
                source: NodeId::one(),
                destination: NodeId::one(),
            }))
        );
    }
}
