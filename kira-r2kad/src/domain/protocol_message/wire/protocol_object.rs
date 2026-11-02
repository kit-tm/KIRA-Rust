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
