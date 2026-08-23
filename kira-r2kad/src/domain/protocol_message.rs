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
use bitflags::bitflags;
use derive_more::Display;

use crate::domain::{
    Contact,
    INVALID_SSN,
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
#[doc(inline)]
pub use dht::{
    FetchReqData,
    FetchRspData,
    StoreReqData,
    StoreRspData,
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

const PROTOCOL_MSG_FLAG_EXACT: u8 = 1;
const PROTOCOL_MSG_FLAG_ENDSYSTEM: u8 = 1 << 2;
const PROTOCOL_MSG_FLAG_DIAGNOSTIC: u8 = 1 << 6;

bitflags! {
    #[derive(Debug, Default, PartialEq, Eq, Clone, Copy, Hash)]
    #[cfg_attr(
        feature = "serde",
        derive(serde::Deserialize, serde::Serialize),
    )]
    #[cfg_attr(
        feature = "binrw",
        derive(BinRead, BinWrite),
        bw(map = |flags: &Self| flags.bits()),
        br(map = |flags_raw: u8| Self::from_bits_retain(flags_raw))
    )]
    #[non_exhaustive]
    pub struct ProtocolMessageFlags: u8 {
        /// Indicates whether the dest-id is a [NodeId] and is assumed to exist.
        ///
        /// If set to 1, the [NodeId] should exist,
        /// if set to 0, the node with the closest [NodeId] will process the request.
        const Exact = PROTOCOL_MSG_FLAG_EXACT;
        /// Indicates that the originating source node is an end-system that
        /// does not perform routing or forwarding.
        ///
        /// > **Note:** The end-system mode is _not_ implemented.
        const EndSystem = PROTOCOL_MSG_FLAG_ENDSYSTEM; // TODO: Implement end-system mode
        /// Triggers explicit Error Messages instead of dropping messages silently.
        ///
        /// This flag serves mainly debugging purposes.
        const Diagnostic = PROTOCOL_MSG_FLAG_DIAGNOSTIC; // TODO: Support Diagnostic flag

        // The source may set any bits
        const _ = !0;
    }
}

// Protocol Message Kind Constants
const PROTOCOL_MSG_KIND_ULN_HELLO: u8 = 0x01;
const PROTOCOL_MSG_KIND_ULN_DISC_REQ: u8 = 0x03;
const PROTOCOL_MSG_KIND_ULN_DISC_RSP: u8 = 0x04;
const PROTOCOL_MSG_KIND_FIND_NODE_REQ: u8 = 0x09;
const PROTOCOL_MSG_KIND_FIND_NODE_RSP: u8 = 0x0a;
const PROTOCOL_MSG_KIND_QUERY_ROUTE_REQ: u8 = 0x0b;
const PROTOCOL_MSG_KIND_QUERY_ROUTE_RSP: u8 = 0x0c;
const PROTOCOL_MSG_KIND_UPDATE_ROUTE_REQ: u8 = 0x11;
const PROTOCOL_MSG_KIND_PROBE_REQ: u8 = 0x21;
const PROTOCOL_MSG_KIND_PROBE_RSP: u8 = 0x22;
const PROTOCOL_MSG_KIND_ERROR: u8 = 0x70;
const PROTOCOL_MSG_KIND_PATH_SETUP_REQ: u8 = 0x81;
const PROTOCOL_MSG_KIND_PATH_SETUP_RSP: u8 = 0x82;
const PROTOCOL_MSG_KIND_PATH_TEARDOWN_REQ: u8 = 0x83;
const PROTOCOL_MSG_KIND_STORE_REQ: u8 = 0xa1;
const PROTOCOL_MSG_KIND_STORE_RSP: u8 = 0xa2;
const PROTOCOL_MSG_KIND_FETCH_REQ: u8 = 0xa3;
const PROTOCOL_MSG_KIND_FETCH_RSP: u8 = 0xa4;

/// Enumeration containing all supported KIRA protocol messages kinds.
#[derive(Debug, Display, PartialEq, Eq, Clone, Copy, Hash)]
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
pub enum ProtocolMessageKind {
    ULNHello,
    ULNDiscReq,
    ULNDiscRsp,
    FindNodeReq,
    FindNodeRsp,
    QueryRouteReq,
    QueryRouteRsp,
    UpdateRouteReq,
    ProbeReq,
    ProbeRsp,
    Error,
    PathSetupReq,
    PathSetupRsp,
    PathTeardownReq,
    StoreReq,
    StoreRsp,
    FetchReq,
    FetchRsp,
    Other(u8),
}

impl From<u8> for ProtocolMessageKind {
    fn from(raw_kind: u8) -> Self {
        match raw_kind {
            PROTOCOL_MSG_KIND_ULN_HELLO => ProtocolMessageKind::ULNHello,
            PROTOCOL_MSG_KIND_ULN_DISC_REQ => ProtocolMessageKind::ULNDiscReq,
            PROTOCOL_MSG_KIND_ULN_DISC_RSP => ProtocolMessageKind::ULNDiscRsp,
            PROTOCOL_MSG_KIND_FIND_NODE_REQ => ProtocolMessageKind::FindNodeReq,
            PROTOCOL_MSG_KIND_FIND_NODE_RSP => ProtocolMessageKind::FindNodeRsp,
            PROTOCOL_MSG_KIND_QUERY_ROUTE_REQ => ProtocolMessageKind::QueryRouteReq,
            PROTOCOL_MSG_KIND_QUERY_ROUTE_RSP => ProtocolMessageKind::QueryRouteRsp,
            PROTOCOL_MSG_KIND_UPDATE_ROUTE_REQ => ProtocolMessageKind::UpdateRouteReq,
            PROTOCOL_MSG_KIND_PROBE_REQ => ProtocolMessageKind::ProbeReq,
            PROTOCOL_MSG_KIND_PROBE_RSP => ProtocolMessageKind::ProbeRsp,
            PROTOCOL_MSG_KIND_ERROR => ProtocolMessageKind::Error,
            PROTOCOL_MSG_KIND_PATH_SETUP_REQ => ProtocolMessageKind::PathSetupReq,
            PROTOCOL_MSG_KIND_PATH_SETUP_RSP => ProtocolMessageKind::PathSetupRsp,
            PROTOCOL_MSG_KIND_PATH_TEARDOWN_REQ => ProtocolMessageKind::PathTeardownReq,
            PROTOCOL_MSG_KIND_STORE_REQ => ProtocolMessageKind::StoreReq,
            PROTOCOL_MSG_KIND_STORE_RSP => ProtocolMessageKind::StoreRsp,
            PROTOCOL_MSG_KIND_FETCH_REQ => ProtocolMessageKind::FetchReq,
            PROTOCOL_MSG_KIND_FETCH_RSP => ProtocolMessageKind::FetchRsp,
            other => ProtocolMessageKind::Other(other),
        }
    }
}

impl From<ProtocolMessageKind> for u8 {
    fn from(kind: ProtocolMessageKind) -> Self {
        match kind {
            ProtocolMessageKind::ULNHello => PROTOCOL_MSG_KIND_ULN_HELLO,
            ProtocolMessageKind::ULNDiscReq => PROTOCOL_MSG_KIND_ULN_DISC_REQ,
            ProtocolMessageKind::ULNDiscRsp => PROTOCOL_MSG_KIND_ULN_DISC_RSP,
            ProtocolMessageKind::FindNodeReq => PROTOCOL_MSG_KIND_FIND_NODE_REQ,
            ProtocolMessageKind::FindNodeRsp => PROTOCOL_MSG_KIND_FIND_NODE_RSP,
            ProtocolMessageKind::QueryRouteReq => PROTOCOL_MSG_KIND_QUERY_ROUTE_REQ,
            ProtocolMessageKind::QueryRouteRsp => PROTOCOL_MSG_KIND_QUERY_ROUTE_RSP,
            ProtocolMessageKind::UpdateRouteReq => PROTOCOL_MSG_KIND_UPDATE_ROUTE_REQ,
            ProtocolMessageKind::ProbeReq => PROTOCOL_MSG_KIND_PROBE_REQ,
            ProtocolMessageKind::ProbeRsp => PROTOCOL_MSG_KIND_PROBE_RSP,
            ProtocolMessageKind::Error => PROTOCOL_MSG_KIND_ERROR,
            ProtocolMessageKind::PathSetupReq => PROTOCOL_MSG_KIND_PATH_SETUP_REQ,
            ProtocolMessageKind::PathSetupRsp => PROTOCOL_MSG_KIND_PATH_SETUP_RSP,
            ProtocolMessageKind::PathTeardownReq => PROTOCOL_MSG_KIND_PATH_TEARDOWN_REQ,
            ProtocolMessageKind::StoreReq => PROTOCOL_MSG_KIND_STORE_REQ,
            ProtocolMessageKind::StoreRsp => PROTOCOL_MSG_KIND_STORE_RSP,
            ProtocolMessageKind::FetchReq => PROTOCOL_MSG_KIND_FETCH_REQ,
            ProtocolMessageKind::FetchRsp => PROTOCOL_MSG_KIND_FETCH_RSP,
            ProtocolMessageKind::Other(other) => other,
        }
    }
}

const PROTOCOL_OBJECT_TYPE_SOURCE_ROUTE: u8 = 0x01;
const PROTOCOL_OBJECT_TYPE_NOT_VIA_LIST: u8 = 0x02;
const PROTOCOL_OBJECT_TYPE_CONTACT_LIST: u8 = 0x03;
const PROTOCOL_OBJECT_TYPE_RTABLE_REQUEST: u8 = 0x04;
const PROTOCOL_OBJECT_TYPE_RTABLE: u8 = 0x05;
const PROTOCOL_OBJECT_TYPE_RTABLE_UPDATE_INFO: u8 = 0x06;
const PROTOCOL_OBJECT_TYPE_ERROR_DATA: u8 = 0x07;
const PROTOCOL_OBJECT_TYPE_STORE_REQ_DATA: u8 = 0x80;
const PROTOCOL_OBJECT_TYPE_STORE_RSP_DATA: u8 = 0x81;
const PROTOCOL_OBJECT_TYPE_FETCH_REQ_DATA: u8 = 0x82;
const PROTOCOL_OBJECT_TYPE_FETCH_RSP_DATA: u8 = 0x83;

/// Object types for protocol message payload objects (see draft section 4.4.2).
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(from = "u8", into = "u8")
)]
#[non_exhaustive]
pub enum ProtocolObjectType {
    SourceRoute,
    NotViaList,
    ContactList,
    RTableRequest,
    RTable,
    RTableUpdateInfo,
    ErrorData,
    StoreReqData,
    StoreRspData,
    FetchReqData,
    FetchRspData,
    Other(u8),
}

impl From<u8> for ProtocolObjectType {
    fn from(value: u8) -> Self {
        match value {
            PROTOCOL_OBJECT_TYPE_SOURCE_ROUTE => Self::SourceRoute,
            PROTOCOL_OBJECT_TYPE_NOT_VIA_LIST => Self::NotViaList,
            PROTOCOL_OBJECT_TYPE_CONTACT_LIST => Self::ContactList,
            PROTOCOL_OBJECT_TYPE_RTABLE_REQUEST => Self::RTableRequest,
            PROTOCOL_OBJECT_TYPE_RTABLE => Self::RTable,
            PROTOCOL_OBJECT_TYPE_RTABLE_UPDATE_INFO => Self::RTableUpdateInfo,
            PROTOCOL_OBJECT_TYPE_ERROR_DATA => Self::ErrorData,
            PROTOCOL_OBJECT_TYPE_STORE_REQ_DATA => Self::StoreReqData,
            PROTOCOL_OBJECT_TYPE_STORE_RSP_DATA => Self::StoreRspData,
            PROTOCOL_OBJECT_TYPE_FETCH_REQ_DATA => Self::FetchReqData,
            PROTOCOL_OBJECT_TYPE_FETCH_RSP_DATA => Self::FetchRspData,
            _ => Self::Other(value),
        }
    }
}

impl From<ProtocolObjectType> for u8 {
    fn from(value: ProtocolObjectType) -> Self {
        match value {
            ProtocolObjectType::SourceRoute => PROTOCOL_OBJECT_TYPE_SOURCE_ROUTE,
            ProtocolObjectType::NotViaList => PROTOCOL_OBJECT_TYPE_NOT_VIA_LIST,
            ProtocolObjectType::ContactList => PROTOCOL_OBJECT_TYPE_CONTACT_LIST,
            ProtocolObjectType::RTableRequest => PROTOCOL_OBJECT_TYPE_RTABLE_REQUEST,
            ProtocolObjectType::RTable => PROTOCOL_OBJECT_TYPE_RTABLE,
            ProtocolObjectType::RTableUpdateInfo => PROTOCOL_OBJECT_TYPE_RTABLE_UPDATE_INFO,
            ProtocolObjectType::ErrorData => PROTOCOL_OBJECT_TYPE_ERROR_DATA,
            ProtocolObjectType::StoreReqData => PROTOCOL_OBJECT_TYPE_STORE_REQ_DATA,
            ProtocolObjectType::StoreRspData => PROTOCOL_OBJECT_TYPE_STORE_RSP_DATA,
            ProtocolObjectType::FetchReqData => PROTOCOL_OBJECT_TYPE_FETCH_REQ_DATA,
            ProtocolObjectType::FetchRspData => PROTOCOL_OBJECT_TYPE_FETCH_RSP_DATA,
            ProtocolObjectType::Other(object_type) => object_type,
        }
    }
}

/// Header that precedes every payload object.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct CommonObjectHeader {
    pub object_type: ProtocolObjectType,
    pub object_length: u16,
}

impl CommonObjectHeader {
    pub fn new(object_type: ProtocolObjectType, object_length: u16) -> Self {
        Self {
            object_type,
            object_length,
        }
    }
}

const RTABLE_REQUEST_TYPE_NONE: u8 = 0x00;
const RTABLE_REQUEST_TYPE_CONTACTS_ONLY: u8 = 0x01;
const RTABLE_REQUEST_TYPE_OVERLAY_NEIGHBORS: u8 = 0x02;
const RTABLE_REQUEST_TYPE_OVERLAY_NEIGHBORS_SOURCE: u8 = 0x03;
const RTABLE_REQUEST_TYPE_ULN_VICINITY: u8 = 0x04;

/// Values for `rtable-request-type-object` (draft section 4.4.2.5).
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Deserialize, serde::Serialize),
    serde(from = "u8", into = "u8")
)]
#[non_exhaustive]
pub enum RTableRequestTypeValue {
    None,
    ContactsOnly,
    OverlayNeighbors,
    OverlayNeighborsSource,
    ULNVicinity,
    Other(u8),
}

impl From<u8> for RTableRequestTypeValue {
    fn from(value: u8) -> Self {
        match value {
            RTABLE_REQUEST_TYPE_NONE => Self::None,
            RTABLE_REQUEST_TYPE_CONTACTS_ONLY => Self::ContactsOnly,
            RTABLE_REQUEST_TYPE_OVERLAY_NEIGHBORS => Self::OverlayNeighbors,
            RTABLE_REQUEST_TYPE_OVERLAY_NEIGHBORS_SOURCE => Self::OverlayNeighborsSource,
            RTABLE_REQUEST_TYPE_ULN_VICINITY => Self::ULNVicinity,
            _ => Self::Other(value),
        }
    }
}

impl From<RTableRequestTypeValue> for u8 {
    fn from(value: RTableRequestTypeValue) -> Self {
        match value {
            RTableRequestTypeValue::None => RTABLE_REQUEST_TYPE_NONE,
            RTableRequestTypeValue::ContactsOnly => RTABLE_REQUEST_TYPE_CONTACTS_ONLY,
            RTableRequestTypeValue::OverlayNeighbors => RTABLE_REQUEST_TYPE_OVERLAY_NEIGHBORS,
            RTableRequestTypeValue::OverlayNeighborsSource => {
                RTABLE_REQUEST_TYPE_OVERLAY_NEIGHBORS_SOURCE
            }
            RTableRequestTypeValue::ULNVicinity => RTABLE_REQUEST_TYPE_ULN_VICINITY,
            RTableRequestTypeValue::Other(req_type) => req_type,
        }
    }
}

/// Common Header Structure
#[derive(Debug, PartialEq, Eq, Clone, Display)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "binrw", derive(BinRead, BinWrite), brw(big))]
#[display("v={} t={} f={:0x} dst={} src={} dom={:x} msg-id={} sseq={} deg={}",
          self.version,
          self.msg_type,
          self.msg_flags,
          self.dest_id,
          self.src_node_id,
          self.domain_id,
          self.msg_id,
          self.state_seq_num,
          self.src_node_degree,
)]
pub struct CommonHeader {
    version: u8,
    msg_type: ProtocolMessageKind,
    msg_flags: ProtocolMessageFlags,
    msg_length: u16,
    #[cfg_attr(feature = "binrw", br(map = |bytes: [u8; NodeId::SIZE]| NodeId::from(bytes)))]
    #[cfg_attr(feature = "binrw", bw(map = |id: &NodeId| id.to_be_bytes()))]
    dest_id: NodeId,
    #[cfg_attr(feature = "binrw", br(map = |bytes: [u8; NodeId::SIZE]| NodeId::from(bytes)))]
    #[cfg_attr(feature = "binrw", bw(map = |id: &NodeId| id.to_be_bytes()))]
    src_node_id: NodeId,
    domain_id: u64,
    msg_id: Nonce,
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
        msgid: Option<Nonce>,
        stateseqnum: Option<u32>,
        src_node_degree: usize,
    ) -> Self {
        Self {
            version: Self::KIRA_PROTOCOL_VERSION,
            msg_type,
            msg_flags: ProtocolMessageFlags::default(),
            msg_length: 1 + 1 + 1 + 2 + 14 + 14 + 8 + 8 + 4 + 2, // common header length
            dest_id: dst,
            src_node_id: src,
            domain_id: 0,
            msg_id: if let Some(msg_id) = msgid {
                msg_id
            } else {
                Nonce::random()
            },
            state_seq_num: if let Some(stateseqnumber) = stateseqnum {
                stateseqnumber
            } else {
                INVALID_SSN
            },
            src_node_degree: if src_node_degree < u16::MAX as usize {
                src_node_degree as u16
            } else {
                u16::MAX
            },
        }
    }

    pub fn set_msg_length(&mut self, msg_len: u16) {
        self.msg_length = msg_len;
    }

    pub fn msg_type(&self) -> ProtocolMessageKind {
        self.msg_type
    }

    pub fn version(&self) -> u8 {
        self.version
    }

    pub fn msg_flags(&self) -> ProtocolMessageFlags {
        self.msg_flags
    }

    pub fn msg_flags_mut(&mut self) -> &mut ProtocolMessageFlags {
        &mut self.msg_flags
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

    pub fn set_msg_id(&mut self, msgid: Nonce) {
        self.msg_id = msgid;
    }

    pub fn msg_id(&self) -> Nonce {
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

    fn msg_flags(&self) -> ProtocolMessageFlags {
        self.common_header().msg_flags
    }

    fn msg_flags_mut(&mut self) -> &mut ProtocolMessageFlags {
        &mut self.common_header_mut().msg_flags
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

    fn set_msg_id(&mut self, msgid: Nonce) {
        self.common_header_mut().set_msg_id(msgid);
    }

    fn msg_id(&self) -> Nonce {
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
    ULNHello(CommonHeader),
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
    StoreReq(ReqRspMessage<StoreReqData<LHTInput>>),
    StoreRsp(ReqRspMessage<StoreRspData>),
    FetchReq(ReqRspMessage<FetchReqData>),
    FetchRsp(ReqRspMessage<FetchRspData<LHTOutput>>),
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

    pub fn not_via(&self) -> Option<&NotViaList> {
        match self {
            Self::ULNHello(_) => None,
            Self::ULNDiscReq(req) => req.not_via.as_ref(),
            Self::ULNDiscRsp(req) => req.not_via.as_ref(),
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
