//! Data types for all protocol messages and wrapped in the central enumeration [ProtocolMessage].

use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::num::NonZeroU64;

use crate::domain::{Contact, Link, NodeId, NotVia, StateSeqNr};
use crate::domain::dht::DHTData;
use crate::messaging::source_route::SourceRoute;
use crate::messaging::dht_messaging::{FetchReqData, FetchRspData, StoreReqData, StoreRspData};

/// Randomly generated number to uniquely identify a protocol message and its
/// response.
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

/// Enumeration containing all supported R²/Kad protocol messages.
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
    ProbeReq(ReqRspMessage<ProbeReqData>),
    ProbeRsp(ReqRspMessage<ProbeRspData>),
    PathSetupReq(ReqRspMessage<PathSetupReqData>),
    PathTeardownReq(ReqRspMessage<PathTeardownReqData>),
    UpdateRouteReq(UpdateRouteReq),
    Error(ReqRspMessage<ErrorData>),
    StoreReq(ReqRspMessage<StoreReqData<DHTData<u8>>>),
    StoreRsp(ReqRspMessage<StoreRspData>),
    FetchRep(ReqRspMessage<FetchReqData>),
    FetchRsp(ReqRspMessage<FetchRspData<DHTData<u8>>>),
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
            Self::ProbeReq(req) => Some(&mut req.source_route),
            Self::ProbeRsp(req) => Some(&mut req.source_route),
            Self::PathSetupReq(req) => Some(&mut req.source_route),
            Self::PathTeardownReq(req) => Some(&mut req.source_route),
            Self::UpdateRouteReq(req) => Some(&mut req.source_route),
            Self::StoreReq(req) => Some(&mut req.source_route),
            Self::StoreRsp(req) => Some(&mut req.source_route),
            Self::FetchRep(req) => Some(&mut req.source_route),
            Self::FetchRsp(req) => Some(&mut req.source_route)
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
            Self::ProbeReq(req) => Some(&req.source_route),
            Self::ProbeRsp(req) => Some(&req.source_route),
            Self::PathSetupReq(req) => Some(&req.source_route),
            Self::PathTeardownReq(req) => Some(&req.source_route),
            Self::UpdateRouteReq(req) => Some(&req.source_route),
            Self::StoreReq(req) => Some(&req.source_route),
            Self::StoreRsp(req) => Some(&req.source_route),
            Self::FetchRep(req) => Some(&req.source_route),
            Self::FetchRsp(req) => Some(&req.source_route)
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
            Self::ProbeReq(req) => Some(req.destination()),
            Self::ProbeRsp(req) => Some(req.destination()),
            Self::PathSetupReq(req) => Some(req.destination()),
            Self::PathTeardownReq(req) => Some(req.destination()),
            Self::UpdateRouteReq(req) => Some(req.source_route.destination()),
            Self::StoreReq(req) => Some(req.source_route.destination()),
            Self::StoreRsp(req) => Some(req.source_route.destination()),
            Self::FetchRep(req) => Some(req.source_route.destination()),
            Self::FetchRsp(req) => Some(req.source_route.destination())
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
            Self::ProbeReq(req) => Some(&req.nonce),
            Self::ProbeRsp(req) => Some(&req.nonce),
            Self::PathSetupReq(req) => Some(&req.nonce),
            Self::PathTeardownReq(req) => Some(&req.nonce),
            Self::UpdateRouteReq(_) => None,
            Self::StoreReq(req) => Some(&req.nonce),
            Self::StoreRsp(req) => Some(&req.nonce),
            Self::FetchRep(req) => Some(&req.nonce),
            Self::FetchRsp(req) => Some(&req.nonce)
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
            Self::Error(ReqRspMessage {
                data: ErrorData::DeadEnd,
                source_route,
                ..
            }) => source_route.source(),
            Self::Error(ReqRspMessage {
                data: ErrorData::SegmentFailure { source, .. },
                ..
            }) => source,
            Self::ProbeReq(req) => req.source(),
            Self::ProbeRsp(req) => req.source(),
            Self::PathSetupReq(req) => req.source(),
            Self::PathTeardownReq(req) => req.source(),
            Self::UpdateRouteReq(req) => req.source_route.source(),
            ProtocolMessage::StoreReq(req) => req.source(),
            ProtocolMessage::StoreRsp(req) => req.source(),
            ProtocolMessage::FetchRep(req) => req.source(),
            ProtocolMessage::FetchRsp(req) => req.source()
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
            Self::ProbeReq(req) => &req.source_state_seq_nr,
            Self::ProbeRsp(req) => &req.source_state_seq_nr,
            Self::PathSetupReq(req) => &req.source_state_seq_nr,
            Self::PathTeardownReq(req) => &req.source_state_seq_nr,
            Self::UpdateRouteReq(req) => &req.source_state_seq_nr,
            ProtocolMessage::StoreReq(req) => &req.source_state_seq_nr,
            ProtocolMessage::StoreRsp(req) => &req.source_state_seq_nr,
            ProtocolMessage::FetchRep(req) => &req.source_state_seq_nr,
            ProtocolMessage::FetchRsp(req) => &req.source_state_seq_nr
        }
    }

    pub fn not_via(&self) -> Option<&HashSet<NotVia>> {
        match self {
            Self::Hello(_) => None,
            Self::PNDiscReq(req) => Some(&req.not_via),
            Self::PNDiscRsp(req) => Some(&req.not_via),
            Self::QueryRouteReq(req) => Some(&req.not_via),
            Self::QueryRouteRsp(req) => Some(&req.not_via),
            Self::FindNodeReq(req) => Some(&req.not_via),
            Self::FindNodeRsp(req) => Some(&req.not_via),
            Self::Error(req) => Some(&req.not_via),
            Self::ProbeReq(req) => Some(&req.not_via),
            Self::ProbeRsp(req) => Some(&req.not_via),
            Self::PathSetupReq(req) => Some(&req.not_via),
            Self::PathTeardownReq(req) => Some(&req.not_via),
            Self::UpdateRouteReq(req) => Some(&req.not_via),
            ProtocolMessage::StoreReq(req) => Some(&req.not_via),
            ProtocolMessage::StoreRsp(req) => Some(&req.not_via),
            ProtocolMessage::FetchRep(req) => Some(&req.not_via),
            ProtocolMessage::FetchRsp(req) => Some(&req.not_via)
        }
    }

    /// Current hop of the message.
    ///
    /// Is only [Option::None] if the message has no source route (PNHello).
    pub fn current_hop(&self) -> Option<&NodeId> {
        self.source_route().map(|sr| sr.current_hop())
    }

    pub fn previous_hop(&self) -> &NodeId {
        self.source_route()
            .map(|sr| sr.prev_hop())
            .unwrap_or_else(|| self.source())
    }
}

/// Data struct representing the PNHello protocol message only exchanged
/// between physical neighbors.
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
    pub not_via: HashSet<NotVia>,
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
        self.source_route.destination()
    }
}

/// Data struct representing the ProbeReq data protocol message.
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct ProbeReqData;

impl From<ReqRspMessage<ProbeReqData>> for ProtocolMessage {
    fn from(message: ReqRspMessage<ProbeReqData>) -> Self {
        Self::ProbeReq(message)
    }
}

/// Data struct representing the ProbeRsp data protocol message.
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct ProbeRspData;

impl From<ReqRspMessage<ProbeRspData>> for ProtocolMessage {
    fn from(message: ReqRspMessage<ProbeRspData>) -> Self {
        Self::ProbeRsp(message)
    }
}

/// Data struct representing the PathSetupReq protocol message.
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct PathSetupReqData;

impl From<ReqRspMessage<PathSetupReqData>> for ProtocolMessage {
    fn from(message: ReqRspMessage<PathSetupReqData>) -> Self {
        Self::PathSetupReq(message)
    }
}

/// Data struct representing the PathTeardownReq protocol message.
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct PathTeardownReqData;

impl From<ReqRspMessage<PathTeardownReqData>> for ProtocolMessage {
    fn from(message: ReqRspMessage<PathTeardownReqData>) -> Self {
        Self::PathTeardownReq(message)
    }
}

/// Data struct representing the UpdateRouteReq protocol message.
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct UpdateRouteReq {
    pub source_state_seq_nr: StateSeqNr,
    pub not_via: HashSet<NotVia>,
    pub contact_actions: HashMap<Contact, RouteUpdate>,
    /// Source Path to the next overlay Hop.
    ///
    /// This way if the source route is too large, it can be split and transmitted
    /// through fragments.
    /// For IPv6 additional Fragment Headers may be used.
    pub source_route: SourceRoute,
}

/// Data type representing the action performed on a contact.
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum RouteUpdate {
    Removed,
    Updated,
}

impl From<UpdateRouteReq> for ProtocolMessage {
    fn from(message: UpdateRouteReq) -> Self {
        Self::UpdateRouteReq(message)
    }
}

/// Data type representing a protocol message which only contains a part
/// of the routing table.
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct RTableData {
    pub contacts: Vec<Contact>,
}

/// Data struct representing a QueryRouteReq protocol message.
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct QueryRouteReqData {
    pub query_type: QueryRouteType,
}

/// Data struct representing the type of a QueryRouteReq.
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum QueryRouteType {
    PhysicalNeighbors,
    // TODO: Evaluate if this is used and when
    //OverlayNeighbors(NonZeroU64),
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

/// Represents the error data sent in an error message.
///
/// In contrast to other messages the source of the contained source route may not be
/// the node which answered.
/// Depending on the error:
///
/// - `DeadEnd`: The source is the node which answered
/// - `SegmentFailure(Link)`: The source is the destination of the request and the node which
///     answered is the first element in the transmitted link.
///
/// In any case the returned error message contains the node which answered and the node which was
/// the destination of the request.
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum ErrorData {
    /// Returned if a FindNodeReq with `exact=true` doesn't find the target node.
    DeadEnd,
    /// Returned if a segment in a source route is not valid.
    ///
    /// E.g. when forwarding a message and the next hop is not a physical neighbor.
    ///
    /// Contains the link which is invalid.
    SegmentFailure { failed_link: Link, source: NodeId },
}

impl ReqRspMessage<ErrorData> {
    /// Returns the destination of the origin message of this error response.
    pub fn request_destination(&self) -> &NodeId {
        match &self.data {
            ErrorData::DeadEnd => self.source(),
            ErrorData::SegmentFailure { failed_link, .. } => failed_link.first(),
        }
    }
}

impl From<ReqRspMessage<ErrorData>> for ProtocolMessage {
    fn from(message: ReqRspMessage<ErrorData>) -> Self {
        Self::Error(message)
    }
}
