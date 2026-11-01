#[cfg(feature = "binrw")]
use binrw::{
    BinRead,
    BinWrite,
};
use derive_more::Display;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_constants_are_stable() {
        // wire constants must never change for protocol compatibility
        assert_eq!(u8::from(ProtocolMessageKind::ULNHello), 0x01);
        assert_eq!(u8::from(ProtocolMessageKind::ULNDiscReq), 0x03);
        assert_eq!(u8::from(ProtocolMessageKind::ULNDiscRsp), 0x04);
        assert_eq!(u8::from(ProtocolMessageKind::FindNodeReq), 0x09);
        assert_eq!(u8::from(ProtocolMessageKind::FindNodeRsp), 0x0a);
        assert_eq!(u8::from(ProtocolMessageKind::QueryRouteReq), 0x0b);
        assert_eq!(u8::from(ProtocolMessageKind::QueryRouteRsp), 0x0c);
        assert_eq!(u8::from(ProtocolMessageKind::UpdateRouteReq), 0x11);
        assert_eq!(u8::from(ProtocolMessageKind::ProbeReq), 0x21);
        assert_eq!(u8::from(ProtocolMessageKind::ProbeRsp), 0x22);
        assert_eq!(u8::from(ProtocolMessageKind::Error), 0x70);
        assert_eq!(u8::from(ProtocolMessageKind::PathSetupReq), 0x81);
        assert_eq!(u8::from(ProtocolMessageKind::PathSetupRsp), 0x82);
        assert_eq!(u8::from(ProtocolMessageKind::PathTeardownReq), 0x83);
        assert_eq!(u8::from(ProtocolMessageKind::StoreReq), 0xa1);
        assert_eq!(u8::from(ProtocolMessageKind::StoreRsp), 0xa2);
        assert_eq!(u8::from(ProtocolMessageKind::FetchReq), 0xa3);
        assert_eq!(u8::from(ProtocolMessageKind::FetchRsp), 0xa4);
    }

    #[test]
    fn conversion_round_trip_for_all_kinds() {
        let kinds = [
            ProtocolMessageKind::ULNHello,
            ProtocolMessageKind::ULNDiscReq,
            ProtocolMessageKind::ULNDiscRsp,
            ProtocolMessageKind::FindNodeReq,
            ProtocolMessageKind::FindNodeRsp,
            ProtocolMessageKind::QueryRouteReq,
            ProtocolMessageKind::QueryRouteRsp,
            ProtocolMessageKind::UpdateRouteReq,
            ProtocolMessageKind::ProbeReq,
            ProtocolMessageKind::ProbeRsp,
            ProtocolMessageKind::Error,
            ProtocolMessageKind::PathSetupReq,
            ProtocolMessageKind::PathSetupRsp,
            ProtocolMessageKind::PathTeardownReq,
            ProtocolMessageKind::StoreReq,
            ProtocolMessageKind::StoreRsp,
            ProtocolMessageKind::FetchReq,
            ProtocolMessageKind::FetchRsp,
            ProtocolMessageKind::Other(0x99),
        ];

        for kind in kinds {
            assert_eq!(ProtocolMessageKind::from(u8::from(kind)), kind);
        }
    }
}
