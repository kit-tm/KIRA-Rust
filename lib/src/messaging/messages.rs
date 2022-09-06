use std::fmt::Debug;
use std::num::NonZeroU64;

use crate::domain::{Contact, NodeId, StateSeqNr};
use crate::messaging::source_route::SourceRoute;

#[derive(Debug, PartialEq, Eq, Clone, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
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
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum ProtocolMessage {
    Hello(HelloMessage),
    PNDiscReq(ReqRspMessage<RTableData>),
    PNDiscRsp(ReqRspMessage<RTableData>),
    QueryRouteReq(ReqRspMessage<QueryRouteReqData>),
    QueryRouteRsp(ReqRspMessage<RTableData>),
    FindNodeReq(ReqRspMessage<FindNodeReqData>),
    FindNodeRsp(ReqRspMessage<RTableData>),
    Error(ReqRspMessage<ErrorData>),
}

impl ProtocolMessage {
    pub fn source_route_mut(&mut self) -> Option<&mut SourceRoute> {
        match self {
            Self::Hello(_) => None,
            Self::PNDiscReq(req) => Some(&mut req.source_route),
            Self::PNDiscRsp(req) => Some(&mut req.source_route),
            Self::QueryRouteReq(req) => Some(&mut req.source_route),
            Self::QueryRouteRsp(req) => Some(&mut req.source_route),
            Self::FindNodeReq(req) => Some(&mut req.source_route),
            Self::FindNodeRsp(req) => Some(&mut req.source_route),
            Self::Error(req) => Some(&mut req.source_route),
        }
    }

    pub fn source_route(&self) -> Option<&SourceRoute> {
        match self {
            Self::Hello(_) => None,
            Self::PNDiscReq(req) => Some(&req.source_route),
            Self::PNDiscRsp(req) => Some(&req.source_route),
            Self::QueryRouteReq(req) => Some(&req.source_route),
            Self::QueryRouteRsp(req) => Some(&req.source_route),
            Self::FindNodeReq(req) => Some(&req.source_route),
            Self::FindNodeRsp(req) => Some(&req.source_route),
            Self::Error(req) => Some(&req.source_route),
        }
    }

    /// Next hop overlay nodes [NodeId].
    pub fn destination(&self) -> Option<&NodeId> {
        match self {
            Self::Hello(_) => None,
            Self::PNDiscReq(req) => Some(req.destination()),
            Self::PNDiscRsp(req) => Some(req.destination()),
            Self::QueryRouteReq(req) => Some(req.destination()),
            Self::QueryRouteRsp(req) => Some(req.destination()),
            Self::FindNodeReq(req) => Some(req.destination()),
            Self::FindNodeRsp(req) => Some(req.destination()),
            Self::Error(req) => Some(req.destination()),
        }
    }

    pub fn nonce(&self) -> Option<&Nonce> {
        match self {
            Self::Hello(_) => None,
            Self::PNDiscReq(req) => Some(&req.nonce),
            Self::PNDiscRsp(req) => Some(&req.nonce),
            Self::QueryRouteReq(req) => Some(&req.nonce),
            Self::QueryRouteRsp(req) => Some(&req.nonce),
            Self::FindNodeReq(req) => Some(&req.nonce),
            Self::FindNodeRsp(req) => Some(&req.nonce),
            Self::Error(req) => Some(&req.nonce),
        }
    }

    pub fn source(&self) -> &NodeId {
        match self {
            Self::Hello(req) => &req.source,
            Self::PNDiscReq(req) => req.source(),
            Self::PNDiscRsp(req) => req.source(),
            Self::QueryRouteReq(req) => req.source(),
            Self::QueryRouteRsp(req) => req.source(),
            Self::FindNodeReq(req) => req.source(),
            Self::FindNodeRsp(req) => req.source(),
            Self::Error(req) => req.source(),
        }
    }

    pub fn source_state_seq_nr(&self) -> &StateSeqNr {
        match self {
            Self::Hello(req) => &req.source_state_seq_nr,
            Self::PNDiscReq(req) => &req.source_state_seq_nr,
            Self::PNDiscRsp(req) => &req.source_state_seq_nr,
            Self::QueryRouteReq(req) => &req.source_state_seq_nr,
            Self::QueryRouteRsp(req) => &req.source_state_seq_nr,
            Self::FindNodeReq(req) => &req.source_state_seq_nr,
            Self::FindNodeRsp(req) => &req.source_state_seq_nr,
            Self::Error(req) => &req.source_state_seq_nr,
        }
    }

    pub fn current_hop(&self) -> Option<&NodeId> {
        self.source_route().map(|sr| sr.current_hop())
    }

    pub fn previous_hop(&self) -> &NodeId {
        self.source_route()
            .map(|sr| sr.prev_hop())
            .unwrap_or_else(|| self.source())
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct HelloMessage {
    pub source: NodeId,
    pub source_state_seq_nr: StateSeqNr,
}

impl From<HelloMessage> for ProtocolMessage {
    fn from(message: HelloMessage) -> Self {
        Self::Hello(message)
    }
}

/// In contrary to a [HelloMessage] this type contains a [Nonce] to
/// identify Request and Response Pairs.
///
/// The target has not to be equal to the end of the source route as some protocol messages
/// are routed from overlay hop to overlay hop.
///
/// The information about the source [NodeId] is stored in the `source_route`.
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct ReqRspMessage<T: Debug> {
    pub nonce: Nonce,
    pub source_state_seq_nr: StateSeqNr,
    pub data: T,
    /// Source Path to the next overlay Hop.
    ///
    /// At the end for a reason.
    /// This way if the source route is too large, it can be split and transmitted
    /// through fragments.
    /// For IPv6 additional Fragment Headers may be used.
    pub source_route: SourceRoute,
}

impl<T: Debug> ReqRspMessage<T> {
    pub fn source(&self) -> &NodeId {
        self.source_route.source()
    }

    /// Next hop destination of the request.
    pub fn destination(&self) -> &NodeId {
        self.source_route.target()
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct RTableData {
    pub contacts: Vec<Contact>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct QueryRouteReqData {
    pub query_type: QueryRouteType,
}

#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum QueryRouteType {
    PhysicalNeighbors,
    OverlayNeighbors(NonZeroU64),
}

impl From<ReqRspMessage<QueryRouteReqData>> for ProtocolMessage {
    fn from(message: ReqRspMessage<QueryRouteReqData>) -> Self {
        Self::QueryRouteReq(message)
    }
}

/// The target of the request is located at the destination id of
/// the [ReqRspMessage].
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct FindNodeReqData {
    pub exact: bool,
    /// The range of the neighborhood to include in the RTableObject of the Response.
    ///
    /// This is usually equal to the BUCKET_SIZE.
    pub neighborhood: NonZeroU64,
    /// The target for this request.
    ///
    /// While the destination of a [ProtocolMessage] represents the next hop to route
    /// the message to the target is specific to FindNodeReq.
    ///
    /// Different kinds of values:
    ///
    /// - Random Probing: Randomly generated NodeId
    /// - Path Probing: Same as destination. Specific contact is probed for connectivity.
    /// - Overlay Neighborhood Discovery: NodeId of the current node.
    pub target: NodeId,
}

impl From<ReqRspMessage<FindNodeReqData>> for ProtocolMessage {
    fn from(message: ReqRspMessage<FindNodeReqData>) -> Self {
        Self::FindNodeReq(message)
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum ErrorData {
    /// Returned if a FindNodeReq with `exact=true` doesn't find the target node.
    DeadEnd,
    /// Returned if a segment in a source route is not valid.
    ///
    /// E.g. when forwarding a message and the next hop is not a physical neighbor.
    SegmentFailure,
}

impl From<ReqRspMessage<ErrorData>> for ProtocolMessage {
    fn from(message: ReqRspMessage<ErrorData>) -> Self {
        Self::Error(message)
    }
}
