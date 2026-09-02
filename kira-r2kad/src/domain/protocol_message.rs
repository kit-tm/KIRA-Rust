//! Data types for all protocol messages and wrapped in the central enumeration [ProtocolMessage].

use std::{
    collections::HashMap,
    fmt::{
        Debug,
        Formatter,
    },
    num::NonZeroU64,
};

#[cfg(feature = "binrw")]
use binrw::{
    BinRead,
    BinWrite,
};
use derive_more::Display;

use crate::domain::{
    Contact,
    Link,
    NodeId,
    NotViaList,
    SourceRoute,
    StateSeqNr,
    protocol_message::dht::{
        LHTInput,
        LHTOutput,
    },
};

pub mod dht;
pub mod wire;
#[doc(inline)]
pub use self::{
    dht::{
        FetchReqData,
        FetchRspData,
        StoreReqData,
        StoreRspData,
    },
    wire::WireFormatMessage,
    wire::{
        CommonHeader,
        ProtocolMessageFlags,
        ProtocolMessageKind,
    },
};

/// Randomly generated number to uniquely identify a protocol message and its
/// response.
#[derive(Display, PartialEq, Eq, Clone, Hash, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "binrw", derive(BinRead, BinWrite), brw(big))]
#[display("{:x}-{:x}", (self.0 >> 32) as u32, (self.0 & 0xffffffff) as u32)]
pub struct Nonce(u64);

impl Debug for Nonce {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Nonce: {:x}-{:x}",
            (self.0 >> 32) as u32,
            (self.0 & 0xffffffff) as u32
        )
    }
}

impl From<u64> for Nonce {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl From<Nonce> for u64 {
    fn from(value: Nonce) -> u64 {
        value.0
    }
}

impl Nonce {
    /// Creates a random [Nonce].‚
    pub fn random() -> Self {
        Self(rand::random())
    }
}

/// Enumeration containing all supported KIRA protocol messages.
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum ProtocolMessage {
    ULNHello(CommonHeader),
    ULNDiscReq(ULNReqRspMessage<RTableData>),
    ULNDiscRsp(ULNReqRspMessage<RTableData>),
    QueryRouteReq(ReqRspMessage<QueryRouteReqData>),
    QueryRouteRsp(ReqRspMessage<RTableData>),
    FindNodeReq(ReqRspMessage<FindNodeReqData>),
    FindNodeRsp(ReqRspMessage<RTableData>),
    ProbeReq(ReqRspMessage<ProbeReqData>),
    ProbeRsp(ReqRspMessage<ProbeRspData>),
    PathSetupReq(ReqRspMessage<PathSetupReqData>),
    PathTeardownReq(ReqRspMessage<PathTeardownReqData>),
    // TODO: add Rsp for Setup and Teardown and handle them accordingly
    UpdateRouteReq(UpdateRouteReq),
    Error(ReqRspMessage<ErrorData>),
    StoreReq(ReqRspMessage<StoreReqData<LHTInput>>),
    StoreRsp(ReqRspMessage<StoreRspData>),
    FetchReq(ReqRspMessage<FetchReqData>),
    FetchRsp(ReqRspMessage<FetchRspData<LHTOutput>>),
}

impl ProtocolMessage {
    pub fn source_route_mut(&mut self) -> Option<&mut SourceRoute> {
        match self {
            Self::ULNHello(_) | Self::ULNDiscReq(_) | Self::ULNDiscRsp(_) => None,
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
            Self::FetchReq(req) => Some(&mut req.source_route),
            Self::FetchRsp(req) => Some(&mut req.source_route),
        }
    }

    pub fn source_route(&self) -> Option<&SourceRoute> {
        match self {
            Self::ULNHello(_) | Self::ULNDiscReq(_) | Self::ULNDiscRsp(_) => None,
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
            Self::FetchReq(req) => Some(&req.source_route),
            Self::FetchRsp(req) => Some(&req.source_route),
        }
    }

    /// Next hop overlay nodes [NodeId].
    pub fn destination(&self) -> Option<&NodeId> {
        match self {
            Self::ULNHello(_) => None,
            Self::ULNDiscReq(req) => Some(req.destination()),
            Self::ULNDiscRsp(req) => Some(req.destination()),
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
            Self::StoreReq(req) => Some(req.destination()),
            Self::StoreRsp(req) => Some(req.destination()),
            Self::FetchReq(req) => Some(req.destination()),
            Self::FetchRsp(req) => Some(req.destination()),
        }
    }

    pub fn msg_id(&self) -> Option<Nonce> {
        match self {
            Self::ULNHello(_) => None,
            Self::ULNDiscReq(req) => Some(req.common_header.msg_id()),
            Self::ULNDiscRsp(req) => Some(req.common_header.msg_id()),
            Self::QueryRouteReq(req) => Some(req.common_header.msg_id()),
            Self::QueryRouteRsp(req) => Some(req.common_header.msg_id()),
            Self::FindNodeReq(req) => Some(req.common_header.msg_id()),
            Self::FindNodeRsp(req) => Some(req.common_header.msg_id()),
            Self::Error(req) => Some(req.common_header.msg_id()),
            Self::ProbeReq(req) => Some(req.common_header.msg_id()),
            Self::ProbeRsp(req) => Some(req.common_header.msg_id()),
            Self::PathSetupReq(req) => Some(req.common_header.msg_id()),
            Self::PathTeardownReq(req) => Some(req.common_header.msg_id()),
            Self::UpdateRouteReq(_) => None,
            Self::StoreReq(req) => Some(req.common_header.msg_id()),
            Self::StoreRsp(req) => Some(req.common_header.msg_id()),
            Self::FetchReq(req) => Some(req.common_header.msg_id()),
            Self::FetchRsp(req) => Some(req.common_header.msg_id()),
        }
    }

    pub fn source(&self) -> &NodeId {
        match self {
            Self::ULNHello(req) => req.src_node_id(),
            Self::ULNDiscReq(req) => req.source(),
            Self::ULNDiscRsp(req) => req.source(),
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
            ProtocolMessage::FetchReq(req) => req.source(),
            ProtocolMessage::FetchRsp(req) => req.source(),
        }
    }

    pub fn source_state_seq_nr(&self) -> StateSeqNr {
        match self {
            Self::ULNHello(req) => req.state_seq_num(),
            Self::ULNDiscReq(req) => req.common_header.state_seq_num(),
            Self::ULNDiscRsp(req) => req.common_header.state_seq_num(),
            Self::QueryRouteReq(req) => req.common_header.state_seq_num(),
            Self::QueryRouteRsp(req) => req.common_header().state_seq_num(),
            Self::FindNodeReq(req) => req.common_header().state_seq_num(),
            Self::FindNodeRsp(req) => req.common_header().state_seq_num(),
            Self::Error(req) => req.common_header().state_seq_num(),
            Self::ProbeReq(req) => req.common_header().state_seq_num(),
            Self::ProbeRsp(req) => req.common_header().state_seq_num(),
            Self::PathSetupReq(req) => req.common_header().state_seq_num(),
            Self::PathTeardownReq(req) => req.common_header().state_seq_num(),
            Self::UpdateRouteReq(req) => req.common_header().state_seq_num(),
            ProtocolMessage::StoreReq(req) => req.common_header().state_seq_num(),
            ProtocolMessage::StoreRsp(req) => req.common_header().state_seq_num(),
            ProtocolMessage::FetchReq(req) => req.common_header().state_seq_num(),
            ProtocolMessage::FetchRsp(req) => req.common_header().state_seq_num(),
        }
    }

    pub fn not_via(&self) -> Option<&NotViaList> {
        match self {
            Self::ULNHello(_) | Self::ULNDiscReq(_) | Self::ULNDiscRsp(_) => None,
            Self::QueryRouteReq(req) => req.not_via.as_ref(),
            Self::QueryRouteRsp(req) => req.not_via.as_ref(),
            Self::FindNodeReq(req) => req.not_via.as_ref(),
            Self::FindNodeRsp(req) => req.not_via.as_ref(),
            Self::Error(req) => req.not_via.as_ref(),
            Self::ProbeReq(req) => req.not_via.as_ref(),
            Self::ProbeRsp(req) => req.not_via.as_ref(),
            Self::PathSetupReq(req) => req.not_via.as_ref(),
            Self::PathTeardownReq(req) => req.not_via.as_ref(),
            Self::UpdateRouteReq(req) => req.not_via.as_ref(),
            Self::StoreReq(req) => req.not_via.as_ref(),
            Self::StoreRsp(req) => req.not_via.as_ref(),
            Self::FetchReq(req) => req.not_via.as_ref(),
            Self::FetchRsp(req) => req.not_via.as_ref(),
        }
    }

    /// Current hop of the message.
    ///
    /// Is only [Option::None] if the message has no destinct destination ([ULNHello])
    /// Messages that can only be sent to underlay neighbors ([ULNDiscReq], [ULNDiscRsp])
    /// will return their [destination] as current hop.
    /// Messages with a [SourceRoute] return the [current hop] of the [SourceRoute].
    ///
    /// [ULNHello]: ProtocolMessage::ULNHello
    /// [ULNDiscReq]: ProtocolMessage::ULNDiscReq
    /// [ULNDiscRsp]: ProtocolMessage::ULNDiscRsp
    /// [destination]: ProtocolMessage::destination
    /// [current hop]: SourceRoute::current_hop
    pub fn current_hop(&self) -> Option<&NodeId> {
        if matches!(self, Self::ULNHello(_)) {
            return None;
        };

        self.source_route()
            .map(|sr| sr.current_hop())
            .or_else(|| self.destination())
    }

    pub fn previous_hop(&self) -> &NodeId {
        self.source_route()
            .map(|sr| sr.prev_hop())
            .unwrap_or_else(|| self.source())
    }

    pub fn kind(&self) -> ProtocolMessageKind {
        match self {
            Self::ULNHello(_) => ProtocolMessageKind::ULNHello,
            Self::ULNDiscReq(_) => ProtocolMessageKind::ULNDiscReq,
            Self::ULNDiscRsp(_) => ProtocolMessageKind::ULNDiscRsp,
            Self::QueryRouteReq(_) => ProtocolMessageKind::QueryRouteReq,
            Self::QueryRouteRsp(_) => ProtocolMessageKind::QueryRouteRsp,
            Self::FindNodeReq(_) => ProtocolMessageKind::FindNodeReq,
            Self::FindNodeRsp(_) => ProtocolMessageKind::FindNodeRsp,
            Self::ProbeReq(_) => ProtocolMessageKind::ProbeReq,
            Self::ProbeRsp(_) => ProtocolMessageKind::ProbeRsp,
            Self::PathSetupReq(_) => ProtocolMessageKind::PathSetupReq,
            Self::PathTeardownReq(_) => ProtocolMessageKind::PathTeardownReq,
            Self::UpdateRouteReq(_) => ProtocolMessageKind::UpdateRouteReq,
            Self::Error(_) => ProtocolMessageKind::Error,
            Self::StoreReq(_) => ProtocolMessageKind::StoreReq,
            Self::StoreRsp(_) => ProtocolMessageKind::StoreRsp,
            Self::FetchReq(_) => ProtocolMessageKind::FetchReq,
            Self::FetchRsp(_) => ProtocolMessageKind::FetchRsp,
        }
    }
}

impl From<&ProtocolMessage> for ProtocolMessageKind {
    fn from(message: &ProtocolMessage) -> Self {
        message.kind()
    }
}

/// In contrary to a [ULNHello](crate::domain::ProtocolMessage::ULNHello) this type contains a [SourceRoute]
/// and data for request and response pairs
///
/// The target has not to be equal to the end of the source route as some protocol messages
/// are routed from overlay hop to overlay hop.
///
/// The information about the source [NodeId] is stored in the `source_route`.
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct ReqRspMessage<T: Debug> {
    pub common_header: CommonHeader,
    pub data: T,
    pub not_via: Option<NotViaList>,
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

    /// Next overlay hop destination of the request.
    ///
    /// Essentially this is the destination of the source route.
    pub fn destination(&self) -> &NodeId {
        // TODO: coherent renaming of methods destination methods
        // to distinguish between current overlay hop "destination" and final destination
        //
        // Currently we have multiple ambiguous destination methods:
        //
        // - `ReqRspMessage::destination`: overlay destination
        // - `ProtocolMessage::destination`: overlay destination (or None on ULNHello)
        // - `WireFormatMessage::dest_id`: final intended destination
        //      can differ from current overlay hop destination if forwarded via multiple overlay hops
        self.source_route.destination()
    }
}

/// In contrary to a [ULNHello](crate::domain::ProtocolMessage::ULNHello) this type contains data for request and response pairs
///
/// The message can only be sent to underlay neighbors.
/// Therefor, this message does not store any [SourceRoute].
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct ULNReqRspMessage<T: Debug> {
    pub common_header: CommonHeader,
    pub data: T,
    // No SourceRoute since this message can only be sent to underlay neighbors
}

impl<T: Debug> ULNReqRspMessage<T> {
    pub fn source(&self) -> &NodeId {
        self.common_header.src_node_id()
    }

    pub fn destination(&self) -> &NodeId {
        self.common_header.dest_id()
    }
}

impl WireFormatMessage for ProtocolMessage {
    fn common_header(&self) -> &CommonHeader {
        match self {
            Self::ULNHello(common_header) => common_header,
            Self::ULNDiscReq(req) => &req.common_header,
            Self::ULNDiscRsp(req) => &req.common_header,
            Self::QueryRouteReq(req) => &req.common_header,
            Self::QueryRouteRsp(req) => &req.common_header,
            Self::FindNodeReq(req) => &req.common_header,
            Self::FindNodeRsp(req) => &req.common_header,
            Self::Error(req) => &req.common_header,
            Self::ProbeReq(req) => &req.common_header,
            Self::ProbeRsp(req) => &req.common_header,
            Self::PathSetupReq(req) => &req.common_header,
            Self::PathTeardownReq(req) => &req.common_header,
            Self::UpdateRouteReq(req) => &req.common_header,
            Self::StoreReq(req) => &req.common_header,
            Self::StoreRsp(req) => &req.common_header,
            Self::FetchReq(req) => &req.common_header,
            Self::FetchRsp(req) => &req.common_header,
        }
    }

    fn common_header_mut(&mut self) -> &mut CommonHeader {
        match self {
            Self::ULNHello(commonheader) => commonheader,
            Self::ULNDiscReq(req) => req.common_header_mut(),
            Self::ULNDiscRsp(req) => req.common_header_mut(),
            Self::QueryRouteReq(req) => req.common_header_mut(),
            Self::QueryRouteRsp(req) => req.common_header_mut(),
            Self::FindNodeReq(req) => req.common_header_mut(),
            Self::FindNodeRsp(req) => req.common_header_mut(),
            Self::Error(req) => req.common_header_mut(),
            Self::ProbeReq(req) => req.common_header_mut(),
            Self::ProbeRsp(req) => req.common_header_mut(),
            Self::PathSetupReq(req) => req.common_header_mut(),
            Self::PathTeardownReq(req) => req.common_header_mut(),
            Self::UpdateRouteReq(req) => req.common_header_mut(),
            Self::StoreReq(req) => req.common_header_mut(),
            Self::StoreRsp(req) => req.common_header_mut(),
            Self::FetchReq(req) => req.common_header_mut(),
            Self::FetchRsp(req) => req.common_header_mut(),
        }
    }
}

impl<T: Debug> WireFormatMessage for ReqRspMessage<T> {
    fn common_header(&self) -> &CommonHeader {
        &self.common_header
    }

    fn common_header_mut(&mut self) -> &mut CommonHeader {
        &mut self.common_header
    }
}

impl<T: Debug> WireFormatMessage for ULNReqRspMessage<T> {
    fn common_header(&self) -> &CommonHeader {
        &self.common_header
    }

    fn common_header_mut(&mut self) -> &mut CommonHeader {
        &mut self.common_header
    }
}

/// Data struct representing the ProbeReq data protocol message.
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct ProbeReqData;

impl From<ReqRspMessage<ProbeReqData>> for ProtocolMessage {
    fn from(mut message: ReqRspMessage<ProbeReqData>) -> Self {
        // Target of ProbeReq is always assumed to exist
        *message.common_header_mut().msg_flags_mut() |= ProtocolMessageFlags::Exact;

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
    pub common_header: CommonHeader,
    pub not_via: Option<NotViaList>,
    pub contact_actions: HashMap<Contact, RouteUpdateActionType>,
    /// Source Path to the next overlay Hop.
    ///
    /// This way if the source route is too large, it can be split and transmitted
    /// through fragments.
    /// For IPv6 additional Fragment Headers may be used.
    pub source_route: SourceRoute,
}

impl WireFormatMessage for UpdateRouteReq {
    fn common_header(&self) -> &CommonHeader {
        &self.common_header
    }

    fn common_header_mut(&mut self) -> &mut CommonHeader {
        &mut self.common_header
    }
}

const ROUTE_UPDATE_ACTION_TYPE_ANNOUNCE: u8 = 0x00;
const ROUTE_UPDATE_ACTION_TYPE_WITHDRAW: u8 = 0x01;
const ROUTE_UPDATE_ACTION_TYPE_CHANGE: u8 = 0x02;
const ROUTE_UPDATE_ACTION_TYPE_UNREACHABLE: u8 = 0x03;

/// Data type representing the action performed on a contact.
#[derive(Debug, Display, PartialEq, Eq, Copy, Clone)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(from = "u8", into = "u8")
)]
#[cfg_attr(feature = "binrw", derive(BinRead, BinWrite),
        bw(map = |kind: &Self| u8::from(*kind)),
        br(map = |kind_raw: u8| Self::from(kind_raw))
)]
#[display("{_variant}")]
#[non_exhaustive]
pub enum RouteUpdateActionType {
    /// New contact in routing table.
    Announce,
    // Contact deleted from routing table.
    WithDraw,
    /// path has been changed, i.e., improved.
    Change,
    /// Contact is currently not reachable (but not yet removed).
    Unreachable,
    Other(u8),
}

impl From<u8> for RouteUpdateActionType {
    fn from(action_type_raw: u8) -> Self {
        match action_type_raw {
            ROUTE_UPDATE_ACTION_TYPE_ANNOUNCE => Self::Announce,
            ROUTE_UPDATE_ACTION_TYPE_WITHDRAW => Self::WithDraw,
            ROUTE_UPDATE_ACTION_TYPE_CHANGE => Self::Change,
            ROUTE_UPDATE_ACTION_TYPE_UNREACHABLE => Self::Unreachable,
            _ => Self::Other(action_type_raw),
        }
    }
}

impl From<RouteUpdateActionType> for u8 {
    fn from(action_type: RouteUpdateActionType) -> Self {
        match action_type {
            RouteUpdateActionType::Announce => ROUTE_UPDATE_ACTION_TYPE_ANNOUNCE,
            RouteUpdateActionType::WithDraw => ROUTE_UPDATE_ACTION_TYPE_WITHDRAW,
            RouteUpdateActionType::Change => ROUTE_UPDATE_ACTION_TYPE_CHANGE,
            RouteUpdateActionType::Unreachable => ROUTE_UPDATE_ACTION_TYPE_UNREACHABLE,
            RouteUpdateActionType::Other(other_action_type_raw) => other_action_type_raw,
        }
    }
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
    UnderlayNeighbors,
    // TODO: Evaluate if this is used and when
    //OverlayNeighbors(NonZeroU64),
}

impl From<ReqRspMessage<QueryRouteReqData>> for ProtocolMessage {
    fn from(mut message: ReqRspMessage<QueryRouteReqData>) -> Self {
        // Target of QueryRouteReq is always assumed to exist
        *message.common_header_mut().msg_flags_mut() |= ProtocolMessageFlags::Exact;
        Self::QueryRouteReq(message)
    }
}

/// The target of the request is located at the destination id of
/// the [ReqRspMessage].
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct FindNodeReqData {
    /// The range of the neighborhood to include in the RTableObject of the Response.
    ///
    /// This is usually equal to the BUCKET_SIZE.
    pub neighborhood: NonZeroU64,
}

impl ReqRspMessage<FindNodeReqData> {
    /// Indicates whether the destination is assumed to exist.
    pub fn exact(&self) -> bool {
        self.msg_flags().contains(ProtocolMessageFlags::Exact)
    }

    /// Indicate that the destination is assumed to exist.
    pub fn set_exact(&mut self) {
        *self.msg_flags_mut() |= ProtocolMessageFlags::Exact;
    }

    /// The target for this request.
    ///
    /// Different kinds of values:
    ///
    /// - Random Probing: Randomly generated [NodeId]
    /// - Path Probing: Specific [Contact] is probed for connectivity.
    /// - Overlay Neighborhood Discovery: [NodeId] of the current node.
    pub fn target(&self) -> &NodeId {
        self.common_header().dest_id()
    }
}

impl ReqRspMessage<QueryRouteReqData> {
    /// Indicates whether the destination is assumed to exist.
    ///
    /// A [`QueryRouteReq`] should always set the exact flag.
    ///
    /// [`QueryRouteReq`]: ProtocolMessage::QueryRouteReq
    pub fn exact(&self) -> bool {
        self.msg_flags().contains(ProtocolMessageFlags::Exact)
    }
}

impl ReqRspMessage<ProbeReqData> {
    /// Indicates whether the destination is assumed to exist.
    ///
    /// A [`ProbeReq`] should always set the exact flag.
    ///
    /// [`ProbeReq`]: ProtocolMessage::ProbeReq
    pub fn exact(&self) -> bool {
        self.msg_flags().contains(ProtocolMessageFlags::Exact)
    }
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
///   answered is the first element in the transmitted link.
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
    /// E.g. when forwarding a message and the next hop is not a underlay neighbor.
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
