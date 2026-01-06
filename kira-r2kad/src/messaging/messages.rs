//! Data types for all protocol messages and wrapped in the central enumeration [ProtocolMessage].

use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::num::NonZeroU64;

use derive_more::derive::Display;

use crate::domain::{Contact, Link, NodeId, NotVia, StateSeqNr, state_seq_nr};
use crate::messaging::dht::{
    DefaultLHTInput, DefaultLHTOutput, FetchReqData, FetchRspData, StoreReqData, StoreRspData,
};
use crate::messaging::source_route::SourceRoute;
use std::fmt;

/// Randomly generated number to uniquely identify a protocol message and its
/// response.
#[derive(Debug, PartialEq, Eq, Clone, Hash, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct Nonce(u64);

impl fmt::Display for Nonce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
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

#[repr(u8)]
pub enum KiraMsgFlagsBit {
    ExactFlag = 1,
    EndSystemFlag = 1 << 2,
    DiagnosticFlag = 1 << 6,
}

/// Enumeration containing all supported KIRA protocol messages kinds.
#[derive(Debug, Display, PartialEq, Eq, Clone, Copy)]
#[display("{_variant}")]
#[repr(u8)]
pub enum ProtocolMessageKind {
    ULNHello = 0x01,
    ULNDiscReq = 0x03,
    ULNDiscRsp = 0x04,
    FindNodeReq = 0x09,
    FindNodeRsp = 0x0a,
    QueryRouteReq = 0x0b,
    QueryRouteRsp = 0x0c,
    UpdateRouteReq = 0x11,
    ProbeReq = 0x21,
    ProbeRsp = 0x22,
    Error = 0x70,
    PathSetupReq = 0x81,
    PathSetupRsp = 0x82,
    PathTeardownReq = 0x83,
    StoreReq = 0xa1,
    StoreRsp = 0xa2,
    FetchReq = 0xa3,
    FetchRsp = 0xa4,
}

/// Common Header Structure
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct CommonHeader {
    version: u8,
    msg_type: u8,
    msg_flags: u8,
    msg_length: u16,
    dest_id: NodeId,
    src_node_id: NodeId,
    domain_id: u64,
    msg_id: u64,
    state_seq_num: u32,
    src_node_degree: u16,
}

impl CommonHeader {
    const KIRA_PROTOCOL_VERSION: u8 = 0;

    /// create a new common header
    /// if msgid is None it is created randomly
    /// if stateseqnum is None, it is set to INVALID_SSN
    pub fn new(
        msg_type: ProtocolMessageKind,
        src: NodeId,
        dst: NodeId,
        msgid: Option<u64>,
        stateseqnum: Option<u32>,
        src_node_degree: usize,
    ) -> Self {
        Self {
            version: Self::KIRA_PROTOCOL_VERSION,
            msg_type: msg_type as u8,
            msg_flags: 0,
            msg_length: 1 + 1 + 1 + 2 + 14 + 14 + 8 + 8 + 4 + 2, // common header length
            dest_id: dst,
            src_node_id: src,
            domain_id: 0,
            msg_id: if let Some(msg_id) = msgid {
                msg_id
            } else {
                rand::random()
            },
            state_seq_num: if let Some(stateseqnumber) = stateseqnum {
                stateseqnumber
            } else {
                state_seq_nr::INVALID_SSN
            },
            src_node_degree: if src_node_degree < u16::MAX as usize {
                src_node_degree as u16
            } else {
                u16::MAX
            },
        }
    }

    pub fn set_flag(&mut self, flag: KiraMsgFlagsBit) {
        self.msg_flags |= flag as u8;
    }

    pub fn clear_flag(&mut self, flag: KiraMsgFlagsBit) {
        self.msg_flags |= !(flag as u8);
    }

    pub fn set_msg_length(&mut self, msg_len: u16) {
        self.msg_length = msg_len;
    }

    pub fn add_to_msg_length(&mut self, msg_len: u16) {
        if self.msg_length <= u16::MAX - msg_len {
            self.msg_length += msg_len;
        } else {
            panic!(
                "maximum msg length exceeded when trying to add {} bytes to {}",
                msg_len, self.msg_length
            );
        }
    }

    pub fn msg_length(&self) -> u16 {
        self.msg_length
    }

    pub fn set_dest_id(&mut self, dst: NodeId) {
        self.dest_id = dst;
    }

    pub fn dest_id(&self) -> &NodeId {
        &self.dest_id
    }

    pub fn set_src_node_id(&mut self, src: NodeId) {
        self.src_node_id = src;
    }

    pub fn src_node_id(&self) -> &NodeId {
        &self.src_node_id
    }

    pub fn set_domain_id(&mut self, domainid: u64) {
        self.domain_id = domainid
    }

    pub fn domain_id(&self) -> u64 {
        self.domain_id
    }

    pub fn set_msg_id(&mut self, msgid: u64) {
        self.msg_id = msgid;
    }

    pub fn msg_id(&self) -> u64 {
        self.msg_id
    }

    pub fn set_state_seq_num(&mut self, ssn: StateSeqNr) {
        self.state_seq_num = StateSeqNr::into(ssn);
    }

    pub fn state_seq_num(&self) -> StateSeqNr {
        StateSeqNr::from(self.state_seq_num)
    }

    pub fn set_src_node_degree(&mut self, degree: u16) {
        self.src_node_degree = degree;
    }

    pub fn src_node_degree(&self) -> u16 {
        self.src_node_degree
    }
}

pub trait WireFormatMessage {
    fn common_header(&self) -> &CommonHeader;
    fn common_header_mut(&mut self) -> &mut CommonHeader;

    fn set_flag(&mut self, flag: KiraMsgFlagsBit) {
        self.common_header_mut().set_flag(flag);
    }

    fn clear_flag(&mut self, flag: KiraMsgFlagsBit) {
        self.common_header_mut().clear_flag(flag);
    }

    fn set_dest_id(&mut self, dst: NodeId) {
        self.common_header_mut().set_dest_id(dst);
    }

    fn dest_id(&self) -> &NodeId {
        self.common_header().dest_id()
    }

    fn set_src_node_id(&mut self, src: NodeId) {
        self.common_header_mut().set_src_node_id(src);
    }

    fn src_node_id(&self) -> &NodeId {
        self.common_header().src_node_id()
    }

    fn set_domain_id(&mut self, domainid: u64) {
        self.common_header_mut().set_domain_id(domainid);
    }

    fn domain_id(&self) -> u64 {
        self.common_header().domain_id()
    }

    fn set_msg_id(&mut self, msgid: u64) {
        self.common_header_mut().set_msg_id(msgid);
    }

    fn msg_id(&self) -> u64 {
        self.common_header().msg_id()
    }

    fn set_state_seq_num(&mut self, ssn: StateSeqNr) {
        self.common_header_mut().state_seq_num = StateSeqNr::into(ssn);
    }

    fn state_seq_num(&self) -> StateSeqNr {
        self.common_header().state_seq_num()
    }

    fn set_src_node_degree(&mut self, degree: u16) {
        self.common_header_mut().src_node_degree = degree;
    }

    fn src_node_degree(&self) -> u16 {
        self.common_header().src_node_degree()
    }
}

/// Enumeration containing all supported KIRA protocol messages.
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum ProtocolMessage {
    ULNHello(HelloMessage),
    ULNDiscReq(ReqRspMessage<RTableData>),
    ULNDiscRsp(ReqRspMessage<RTableData>),
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
    StoreReq(ReqRspMessage<StoreReqData<DefaultLHTInput>>),
    StoreRsp(ReqRspMessage<StoreRspData>),
    FetchReq(ReqRspMessage<FetchReqData>),
    FetchRsp(ReqRspMessage<FetchRspData<DefaultLHTOutput>>),
}

impl ProtocolMessage {
    pub fn source_route_mut(&mut self) -> Option<&mut SourceRoute> {
        match self {
            Self::ULNHello(_) => None,
            Self::ULNDiscReq(req) => Some(&mut req.source_route),
            Self::ULNDiscRsp(req) => Some(&mut req.source_route),
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
            Self::ULNHello(_) => None,
            Self::ULNDiscReq(req) => Some(&req.source_route),
            Self::ULNDiscRsp(req) => Some(&req.source_route),
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
            Self::ULNDiscReq(req) => Some(Nonce::from(req.common_header.msg_id())),
            Self::ULNDiscRsp(req) => Some(Nonce::from(req.common_header.msg_id())),
            Self::QueryRouteReq(req) => Some(Nonce::from(req.common_header.msg_id())),
            Self::QueryRouteRsp(req) => Some(Nonce::from(req.common_header.msg_id())),
            Self::FindNodeReq(req) => Some(Nonce::from(req.common_header.msg_id())),
            Self::FindNodeRsp(req) => Some(Nonce::from(req.common_header.msg_id())),
            Self::Error(req) => Some(Nonce::from(req.common_header.msg_id())),
            Self::ProbeReq(req) => Some(Nonce::from(req.common_header.msg_id())),
            Self::ProbeRsp(req) => Some(Nonce::from(req.common_header.msg_id())),
            Self::PathSetupReq(req) => Some(Nonce::from(req.common_header.msg_id())),
            Self::PathTeardownReq(req) => Some(Nonce::from(req.common_header.msg_id())),
            Self::UpdateRouteReq(_) => None,
            Self::StoreReq(req) => Some(Nonce::from(req.common_header.msg_id())),
            Self::StoreRsp(req) => Some(Nonce::from(req.common_header.msg_id())),
            Self::FetchReq(req) => Some(Nonce::from(req.common_header.msg_id())),
            Self::FetchRsp(req) => Some(Nonce::from(req.common_header.msg_id())),
        }
    }

    pub fn source(&self) -> &NodeId {
        match self {
            Self::ULNHello(req) => req.common_header.src_node_id(),
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
            Self::ULNHello(req) => req.common_header().state_seq_num(),
            Self::ULNDiscReq(req) => req.common_header().state_seq_num(),
            Self::ULNDiscRsp(req) => req.common_header().state_seq_num(),
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

    pub fn not_via(&self) -> Option<&HashSet<NotVia>> {
        match self {
            Self::ULNHello(_) => None,
            Self::ULNDiscReq(req) => Some(&req.not_via),
            Self::ULNDiscRsp(req) => Some(&req.not_via),
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
            Self::StoreReq(req) => Some(&req.not_via),
            Self::StoreRsp(req) => Some(&req.not_via),
            Self::FetchReq(req) => Some(&req.not_via),
            Self::FetchRsp(req) => Some(&req.not_via),
        }
    }

    // TODO: write documentation how to use
    // and why other "overlay" messages are not listed here
    pub fn overlay_destination(&self) -> Option<&NodeId> {
        match self {
            Self::StoreReq(req) => Some(&req.data.handle),
            Self::FetchReq(req) => Some(&req.data.handle),
            _ => None,
        }
    }

    /// Current hop of the message.
    ///
    /// Is only [Option::None] if the message has no source route (ULNHello).
    pub fn current_hop(&self) -> Option<&NodeId> {
        self.source_route().map(|sr| sr.current_hop())
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

/// Data struct representing the ULNHello protocol message only exchanged
/// between underlay neighbors.
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct HelloMessage {
    pub common_header: CommonHeader,
}

impl From<HelloMessage> for ProtocolMessage {
    fn from(message: HelloMessage) -> Self {
        Self::ULNHello(message)
    }
}

impl WireFormatMessage for HelloMessage {
    fn common_header(&self) -> &CommonHeader {
        &self.common_header
    }
    fn common_header_mut(&mut self) -> &mut CommonHeader {
        &mut self.common_header
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
    pub common_header: CommonHeader,
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

impl<T: Debug> WireFormatMessage for ReqRspMessage<T> {
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
    pub common_header: CommonHeader,
    pub not_via: HashSet<NotVia>,
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

/// Data type representing the action performed on a contact.
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum RouteUpdateActionType {
    Announce,    // new contact in routing table
    WithDraw,    // contact deleted from routing table
    Change,      // path has been changed, i.e., improved
    Unreachable, // contact is currently not reachable
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
