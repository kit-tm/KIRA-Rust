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
    PNDiscRsp(ReqRspMessage<DiscRspData<ID_SIZE>, ID_SIZE>),
    QueryRouteReq(ReqRspMessage<QueryRouteReqData<ID_SIZE>, ID_SIZE>),
    QueryRouteRsp(ReqRspMessage<DiscRspData<ID_SIZE>, ID_SIZE>),
    FindNodeReq(ReqRspMessage<FindNodeReqData, ID_SIZE>),
    FindNodeRsp(ReqRspMessage<DiscRspData<ID_SIZE>, ID_SIZE>),
    Error(ReqRspMessage<ErrorData, ID_SIZE>),
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
    pub nonce: Nonce,
    pub source: NodeId<ID_SIZE>,
    pub destination: NodeId<ID_SIZE>,
    pub data: T,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum RTableReqType {
    ContactsOnly,
    NeighborHood(usize),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct PNDiscReqData<const ID_SIZE: usize> {
    pub req_type: RTableReqType,
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
pub enum DiscRspData<const ID_SIZE: usize> {
    RTable(Vec<Contact<ID_SIZE>>),
    ContactList(Vec<NodeId<ID_SIZE>>),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct QueryRouteReqData<const ID_SIZE: usize> {
    pub req_type: RTableReqType,
}

impl<const ID_SIZE: usize> From<ReqRspMessage<QueryRouteReqData<ID_SIZE>, ID_SIZE>>
    for Message<ID_SIZE>
{
    fn from(message: ReqRspMessage<QueryRouteReqData<ID_SIZE>, ID_SIZE>) -> Self {
        Self::QueryRouteReq(message)
    }
}

/// The target of the request is located at the destination id of
/// the [ReqRspMessage].
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct FindNodeReqData {
    pub req_type: RTableReqType,
}

impl<const ID_SIZE: usize> From<ReqRspMessage<FindNodeReqData, ID_SIZE>> for Message<ID_SIZE> {
    fn from(message: ReqRspMessage<FindNodeReqData, ID_SIZE>) -> Self {
        Self::FindNodeReq(message)
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum ErrorData {
    DeadEnd,
    SegmentFailure,
}
