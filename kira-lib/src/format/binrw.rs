//! Compact binary format of a [ProtocolMessage].
//!
//! Implemented using [binrw].

// TODO: Refactor:
// 1. Provide Serialization and Deserialization based on traits like netlink is doing
// 2. Create dedicated ProtocolObjectType structs
// 3. Provide Serialization and Deserialization on structs by trait
// 5. Either utilize binrw macros on the ProtocolObjectType structs
//    or even support zerocopy (crate, pro: zero-copy, encodes endianess in type system,
//    struct size == bytesize => less magic numbers)
// 6. Don't iterate over whole payload to find each ProtocolObjectType
//    but deserialize them as they come (possibly Vec<ProtocolObject>).
//    Then access by index and check type for each ProtocolObjectType as
//    order is fixed (unlike netlink).

use std::{
    collections::{
        HashMap,
        HashSet,
    },
    error::Error,
    io::{
        Error as IoError,
        ErrorKind,
        Read,
        Seek,
        SeekFrom,
        Write,
    },
    num::NonZeroU64,
    sync::Arc,
};

use binrw::{
    self,
    BinRead,
    BinWrite,
};
use kira_r2kad::domain::{
    Age,
    Contact,
    Link,
    NodeId,
    NotVia,
    Path,
    ProtocolMessageKind,
    SafeStateSeqNr,
    SourceRoute,
    protocol_message::{
        CommonHeader,
        ErrorData,
        FindNodeReqData,
        PathSetupReqData,
        PathTeardownReqData,
        ProbeReqData,
        ProbeRspData,
        ProtocolMessage,
        QueryRouteReqData,
        QueryRouteType,
        RTableData,
        ReqRspMessage,
        RouteUpdateActionType,
        ULNReqRspMessage,
        UpdateRouteReq,
        dht::{
            FetchErr,
            FetchReqData,
            FetchRspData,
            LHTInput,
            LHTOutput,
            StoreErr,
            StoreOk,
            StoreReqData,
            StoreRspData,
        },
        wire::{
            CommonObjectHeader,
            ProtocolObjectType,
            RTableRequestTypeValue,
        },
    },
};

const HEADER_LEN: usize = 55; // KIRA header size (bytes)
const ERROR_DEAD_END: u8 = 0x0a;
const ERROR_SEGMENT_FAILURE: u8 = 0x05;
const STORE_STATUS_CREATED: u8 = 0x00;
const STORE_STATUS_INSERTED: u8 = 0x01;
const STORE_STATUS_UPDATED: u8 = 0x02;
const FETCH_STATUS_OK: u8 = 0x00;
const FETCH_STATUS_NOT_FOUND: u8 = 0x01;

/// Deserializes a [ProtocolMessage].
pub fn from_reader<R: Read>(mut reader: R) -> Result<ProtocolMessage, Box<dyn Error>> {
    let mut buf = [0u8; HEADER_LEN];
    reader.read_exact(&mut buf)?;

    let mut cursor = binrw::io::Cursor::new(&buf);
    let header = CommonHeader::read_options(&mut cursor, binrw::Endian::Big, ())?;
    match header.msg_type() {
        ProtocolMessageKind::ULNHello => Ok(ProtocolMessage::ULNHello(header)),
        ProtocolMessageKind::ULNDiscReq => deserialize_uln_disc_req(header, &mut reader),
        ProtocolMessageKind::ULNDiscRsp => deserialize_uln_disc_rsp(header, &mut reader),

        ProtocolMessageKind::QueryRouteReq => deserialize_query_route_req(header, &mut reader),
        ProtocolMessageKind::QueryRouteRsp => deserialize_query_route_rsp(header, &mut reader),

        ProtocolMessageKind::FindNodeReq => deserialize_find_node_req(header, &mut reader),
        ProtocolMessageKind::FindNodeRsp => deserialize_find_node_rsp(header, &mut reader),

        ProtocolMessageKind::UpdateRouteReq => deserialize_update_route_req(header, &mut reader),

        ProtocolMessageKind::ProbeReq => deserialize_probe_req(header, &mut reader),
        ProtocolMessageKind::ProbeRsp => deserialize_probe_rsp(header, &mut reader),

        ProtocolMessageKind::PathSetupReq => deserialize_path_setup_req(header, &mut reader),
        ProtocolMessageKind::PathTeardownReq => deserialize_path_teardown_req(header, &mut reader),

        ProtocolMessageKind::Error => deserialize_error(header, &mut reader),

        ProtocolMessageKind::StoreReq => deserialize_store_req(header, &mut reader),
        ProtocolMessageKind::StoreRsp => deserialize_store_rsp(header, &mut reader),
        ProtocolMessageKind::FetchReq => deserialize_fetch_req(header, &mut reader),
        ProtocolMessageKind::FetchRsp => deserialize_fetch_rsp(header, &mut reader),
        ProtocolMessageKind::Other(other_kind) => Err(Box::new(IoError::new(
            ErrorKind::Unsupported,
            format!("msg_type {other_kind:#x}, currently not supported by binrw"),
        ))),
        _ => Err(Box::new(IoError::new(
            ErrorKind::Unsupported,
            "Unexpected ProtocolMessageKind, currently not supported by binrw".to_string(),
        ))),
    }
}

/// Serializes a [ProtocolMessage].
pub fn serialize<W: Write>(
    mut writer: W,
    message: &ProtocolMessage,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    match message {
        ProtocolMessage::ULNHello(header) => {
            let mut cursor = binrw::io::Cursor::new(Vec::with_capacity(HEADER_LEN));
            header.write_options(&mut cursor, binrw::Endian::Big, ())?;
            writer.write_all(cursor.get_ref())?;
            Ok(())
        }
        ProtocolMessage::ULNDiscReq(req) | ProtocolMessage::ULNDiscRsp(req) => {
            serialize_uln_req_rsp_rtable(writer, req)
        }
        ProtocolMessage::QueryRouteRsp(req) | ProtocolMessage::FindNodeRsp(req) => {
            serialize_req_rsp_rtable(writer, req)
        }
        ProtocolMessage::QueryRouteReq(req) => serialize_query_route_req(writer, req),
        ProtocolMessage::FindNodeReq(req) => serialize_find_node_req(writer, req),
        ProtocolMessage::UpdateRouteReq(req) => serialize_update_route_req(writer, req),
        ProtocolMessage::ProbeReq(req) => serialize_probe_req(writer, req),
        ProtocolMessage::ProbeRsp(req) => serialize_probe_rsp(writer, req),
        ProtocolMessage::PathSetupReq(req) => serialize_path_setup_req(writer, req),
        ProtocolMessage::PathTeardownReq(req) => serialize_path_teardown_req(writer, req),
        ProtocolMessage::Error(req) => serialize_error(writer, req),
        ProtocolMessage::StoreReq(req) => serialize_store_req(writer, req),
        ProtocolMessage::StoreRsp(req) => serialize_store_rsp(writer, req),
        ProtocolMessage::FetchReq(req) => serialize_fetch_req(writer, req),
        ProtocolMessage::FetchRsp(req) => serialize_fetch_rsp(writer, req),
        /*_ => Err(Box::new(IoError::new(
            ErrorKind::Unsupported,
            "currently not supported by binrw ",
        ))),*/
    }
}

fn deserialize_probe_req<R: Read>(
    header: CommonHeader,
    reader: &mut R,
) -> Result<ProtocolMessage, Box<dyn Error>> {
    deserialize_req_rsp_no_data(header, reader, ProbeReqData, ProtocolMessage::ProbeReq)
}

fn deserialize_probe_rsp<R: Read>(
    header: CommonHeader,
    reader: &mut R,
) -> Result<ProtocolMessage, Box<dyn Error>> {
    deserialize_req_rsp_no_data(header, reader, ProbeRspData, ProtocolMessage::ProbeRsp)
}

fn deserialize_error<R: Read>(
    header: CommonHeader,
    reader: &mut R,
) -> Result<ProtocolMessage, Box<dyn Error>> {
    let payload = read_payload_bytes(&header, reader)?;
    let parsed = parse_req_rsp_payload_from_bytes(&header, &payload)?;
    let data = parse_error_data_from_bytes(&payload)?;
    let Some(source_route) = parsed.source_route else {
        return Err(Box::new(IoError::new(
            ErrorKind::InvalidData,
            "missing source-route object in Error msg",
        )));
    };

    Ok(ProtocolMessage::Error(ReqRspMessage {
        common_header: header,
        data,
        not_via: Option::from(parsed.not_via),
        source_route,
    }))
}

fn parse_error_data_from_bytes(payload: &[u8]) -> Result<ErrorData, Box<dyn Error>> {
    let mut payload_cursor = binrw::io::Cursor::new(&payload);
    let payload_len = payload.len();
    let mut payload_consumed = 0;
    let mut error_data: Option<ErrorData> = None;

    while payload_consumed < payload_len {
        let CommonObjectHeader {
            object_type,
            object_length,
        } = read_common_object_header(&mut payload_cursor)?;
        let object_length = object_length as usize;

        if payload_consumed + 3 + object_length > payload_len {
            return Err(Box::new(IoError::new(
                ErrorKind::InvalidData,
                format!(
                    "object length exceeds payload (object length = {}, payload length = {}, consumed = {})",
                    object_length, payload_len, payload_consumed
                ),
            )));
        }

        payload_consumed += 3;

        match object_type {
            ProtocolObjectType::ErrorData => {
                if object_length < 1 {
                    return Err(Box::new(IoError::new(
                        ErrorKind::InvalidData,
                        format!(
                            "error-data object too short (length = {}, expected at least 1)",
                            object_length
                        ),
                    )));
                }

                let kind = u8::read_options(&mut payload_cursor, binrw::Endian::Big, ())?;
                match kind {
                    ERROR_DEAD_END => {
                        if object_length != 1 {
                            return Err(Box::new(IoError::new(
                                ErrorKind::InvalidData,
                                format!(
                                    "dead-end error-data object must be exactly 1 byte (length = {})",
                                    object_length
                                ),
                            )));
                        }
                        error_data = Some(ErrorData::DeadEnd);
                    }
                    ERROR_SEGMENT_FAILURE => {
                        let expected_len = 1 + (NodeId::SIZE * 3);
                        if object_length != expected_len {
                            return Err(Box::new(IoError::new(
                                ErrorKind::InvalidData,
                                format!(
                                    "segment-failure error-data object has invalid length (length = {}, expected = {})",
                                    object_length, expected_len
                                ),
                            )));
                        }

                        let mut source = [0u8; NodeId::SIZE];
                        let mut first = [0u8; NodeId::SIZE];
                        let mut second = [0u8; NodeId::SIZE];
                        payload_cursor.read_exact(&mut source)?;
                        payload_cursor.read_exact(&mut first)?;
                        payload_cursor.read_exact(&mut second)?;
                        error_data = Some(ErrorData::SegmentFailure {
                            failed_link: Link::new(NodeId::from(first), NodeId::from(second)),
                            source: NodeId::from(source),
                        });
                    }
                    other => {
                        return Err(Box::new(IoError::new(
                            ErrorKind::InvalidData,
                            format!("unknown error-data kind: {other:#x}"),
                        )));
                    }
                }

                payload_consumed += object_length;
            }
            _ => {
                payload_cursor.seek(SeekFrom::Current(object_length as i64))?;
                payload_consumed += object_length;
            }
        }
    }

    error_data.ok_or_else(|| {
        Box::new(IoError::new(
            ErrorKind::InvalidData,
            format!(
                "missing error-data object in Error message (consumed = {})",
                payload_consumed
            ),
        )) as Box<dyn Error>
    })
}

fn deserialize_path_setup_req<R: Read>(
    header: CommonHeader,
    reader: &mut R,
) -> Result<ProtocolMessage, Box<dyn Error>> {
    deserialize_req_rsp_no_data(
        header,
        reader,
        PathSetupReqData,
        ProtocolMessage::PathSetupReq,
    )
}

fn deserialize_path_teardown_req<R: Read>(
    header: CommonHeader,
    reader: &mut R,
) -> Result<ProtocolMessage, Box<dyn Error>> {
    deserialize_req_rsp_no_data(
        header,
        reader,
        PathTeardownReqData,
        ProtocolMessage::PathTeardownReq,
    )
}

fn deserialize_store_req<R: Read>(
    header: CommonHeader,
    reader: &mut R,
) -> Result<ProtocolMessage, Box<dyn Error>> {
    deserialize_req_rsp_with_payload_data(
        header,
        reader,
        parse_store_req_data_from_bytes,
        ProtocolMessage::StoreReq,
    )
}

fn deserialize_store_rsp<R: Read>(
    header: CommonHeader,
    reader: &mut R,
) -> Result<ProtocolMessage, Box<dyn Error>> {
    deserialize_req_rsp_with_payload_data(
        header,
        reader,
        parse_store_rsp_data_from_bytes,
        ProtocolMessage::StoreRsp,
    )
}

fn deserialize_fetch_req<R: Read>(
    header: CommonHeader,
    reader: &mut R,
) -> Result<ProtocolMessage, Box<dyn Error>> {
    deserialize_req_rsp_with_payload_data(
        header,
        reader,
        parse_fetch_req_data_from_bytes,
        ProtocolMessage::FetchReq,
    )
}

fn deserialize_fetch_rsp<R: Read>(
    header: CommonHeader,
    reader: &mut R,
) -> Result<ProtocolMessage, Box<dyn Error>> {
    deserialize_req_rsp_with_payload_data(
        header,
        reader,
        parse_fetch_rsp_data_from_bytes,
        ProtocolMessage::FetchRsp,
    )
}

fn deserialize_req_rsp_no_data<R: Read, T: std::fmt::Debug, M>(
    header: CommonHeader,
    reader: &mut R,
    data: T,
    msg_type: M,
) -> Result<ProtocolMessage, Box<dyn Error>>
where
    M: FnOnce(ReqRspMessage<T>) -> ProtocolMessage,
{
    let parsed = deserialize_req_rsp_payload(&header, reader)?;
    let Some(source_route) = parsed.source_route else {
        return Err(Box::new(IoError::new(
            ErrorKind::InvalidData,
            "missing source route",
        )));
    };

    Ok(msg_type(ReqRspMessage {
        common_header: header,
        data,
        not_via: Option::from(parsed.not_via),
        source_route,
    }))
}

fn deserialize_req_rsp_with_payload_data<R: Read, T: std::fmt::Debug, P, M>(
    header: CommonHeader,
    reader: &mut R,
    parse_data: P,
    msg_type: M,
) -> Result<ProtocolMessage, Box<dyn Error>>
where
    P: FnOnce(&[u8]) -> Result<T, Box<dyn Error>>,
    M: FnOnce(ReqRspMessage<T>) -> ProtocolMessage,
{
    let payload = read_payload_bytes(&header, reader)?;
    let parsed = parse_req_rsp_payload_from_bytes(&header, &payload)?;
    let data = parse_data(&payload)?;
    let Some(source_route) = parsed.source_route else {
        return Err(Box::new(IoError::new(
            ErrorKind::InvalidData,
            "missing source-route object",
        )));
    };

    Ok(msg_type(ReqRspMessage {
        common_header: header,
        data,
        not_via: Option::from(parsed.not_via),
        source_route,
    }))
}

fn parse_store_req_data_from_bytes(
    payload: &[u8],
) -> Result<StoreReqData<LHTInput>, Box<dyn Error>> {
    let mut payload_cursor = binrw::io::Cursor::new(payload);
    let payload_len = payload.len();
    let mut payload_consumed = 0;

    while payload_consumed < payload_len {
        let CommonObjectHeader {
            object_type,
            object_length,
        } = read_common_object_header(&mut payload_cursor)?;
        let object_length = object_length as usize;

        if payload_consumed + 3 + object_length > payload_len {
            return Err(Box::new(IoError::new(
                ErrorKind::InvalidData,
                format!(
                    "object length exceeds payload (object length = {}, payload length = {}, consumed = {})",
                    object_length, payload_len, payload_consumed
                ),
            )));
        }
        payload_consumed += 3;

        match object_type {
            ProtocolObjectType::StoreReqData => {
                if object_length < NodeId::SIZE + 2 {
                    return Err(Box::new(IoError::new(
                        ErrorKind::InvalidData,
                        format!(
                            "store-req-data object too short (length = {}, expected at least {})",
                            object_length,
                            NodeId::SIZE + 2
                        ),
                    )));
                }

                let mut handle = [0u8; NodeId::SIZE];
                payload_cursor.read_exact(&mut handle)?;

                let data_len =
                    u16::read_options(&mut payload_cursor, binrw::Endian::Big, ())? as usize;
                let min_len = NodeId::SIZE + 2 + data_len;
                let max_len = min_len + 8;
                if object_length != min_len && object_length != max_len {
                    return Err(Box::new(IoError::new(
                        ErrorKind::InvalidData,
                        format!(
                            "invalid store-req-data object length (length = {}, expected = {} or {})",
                            object_length, min_len, max_len
                        ),
                    )));
                }

                let mut data = vec![0u8; data_len];
                payload_cursor.read_exact(&mut data)?;

                let last_accessed_ms = if object_length == min_len {
                    None
                } else {
                    let age_ms = u64::read_options(&mut payload_cursor, binrw::Endian::Big, ())?;
                    Some(Age::from(age_ms))
                };

                return Ok(StoreReqData {
                    handle: NodeId::from(handle),
                    data: Arc::from(data),
                    last_accessed_ms,
                });
            }
            _ => {
                payload_cursor.seek(SeekFrom::Current(object_length as i64))?;
            }
        }

        payload_consumed += object_length;
    }

    Err(Box::new(IoError::new(
        ErrorKind::InvalidData,
        "missing store-req-data object",
    )))
}

fn parse_store_rsp_data_from_bytes(payload: &[u8]) -> Result<StoreRspData, Box<dyn Error>> {
    let mut payload_cursor = binrw::io::Cursor::new(payload);
    let payload_len = payload.len();
    let mut payload_consumed = 0;

    while payload_consumed < payload_len {
        let CommonObjectHeader {
            object_type,
            object_length,
        } = read_common_object_header(&mut payload_cursor)?;
        let object_length = object_length as usize;

        if payload_consumed + 3 + object_length > payload_len {
            return Err(Box::new(IoError::new(
                ErrorKind::InvalidData,
                format!(
                    "object length exceeds payload (object length = {}, payload length = {}, consumed = {})",
                    object_length, payload_len, payload_consumed
                ),
            )));
        }
        payload_consumed += 3;

        match object_type {
            ProtocolObjectType::StoreRspData => {
                if object_length != 1 {
                    return Err(Box::new(IoError::new(
                        ErrorKind::InvalidData,
                        format!(
                            "store-rsp-data object must be exactly 1 byte (length = {})",
                            object_length
                        ),
                    )));
                }

                let status = u8::read_options(&mut payload_cursor, binrw::Endian::Big, ())?;
                let status = match status {
                    STORE_STATUS_CREATED => Ok(StoreOk::Created),
                    STORE_STATUS_INSERTED => Ok(StoreOk::Inserted),
                    STORE_STATUS_UPDATED => Ok(StoreOk::Updated),
                    other => {
                        return Err(Box::new(IoError::new(
                            ErrorKind::InvalidData,
                            format!("invalid store-rsp status: {other:#x}"),
                        )));
                    }
                };

                return Ok(StoreRspData { status });
            }
            _ => {
                payload_cursor.seek(SeekFrom::Current(object_length as i64))?;
            }
        }

        payload_consumed += object_length;
    }

    Err(Box::new(IoError::new(
        ErrorKind::InvalidData,
        "missing store-rsp-data object",
    )))
}

fn parse_fetch_req_data_from_bytes(payload: &[u8]) -> Result<FetchReqData, Box<dyn Error>> {
    let mut payload_cursor = binrw::io::Cursor::new(payload);
    let payload_len = payload.len();
    let mut payload_consumed = 0;

    while payload_consumed < payload_len {
        let CommonObjectHeader {
            object_type,
            object_length,
        } = read_common_object_header(&mut payload_cursor)?;
        let object_length = object_length as usize;

        if payload_consumed + 3 + object_length > payload_len {
            return Err(Box::new(IoError::new(
                ErrorKind::InvalidData,
                format!(
                    "object length exceeds payload (object length = {}, payload length = {}, consumed = {})",
                    object_length, payload_len, payload_consumed
                ),
            )));
        }
        payload_consumed += 3;

        match object_type {
            ProtocolObjectType::FetchReqData => {
                if object_length != NodeId::SIZE {
                    return Err(Box::new(IoError::new(
                        ErrorKind::InvalidData,
                        format!(
                            "fetch-req-data object has invalid length (length = {}, expected = {})",
                            object_length,
                            NodeId::SIZE
                        ),
                    )));
                }

                let mut handle = [0u8; NodeId::SIZE];
                payload_cursor.read_exact(&mut handle)?;

                return Ok(FetchReqData {
                    handle: NodeId::from(handle),
                });
            }
            _ => {
                payload_cursor.seek(SeekFrom::Current(object_length as i64))?;
            }
        }

        payload_consumed += object_length;
    }

    Err(Box::new(IoError::new(
        ErrorKind::InvalidData,
        "missing fetch-req-data object",
    )))
}

fn parse_fetch_rsp_data_from_bytes(
    payload: &[u8],
) -> Result<FetchRspData<LHTOutput>, Box<dyn Error>> {
    let mut payload_cursor = binrw::io::Cursor::new(payload);
    let payload_len = payload.len();
    let mut payload_consumed = 0;

    while payload_consumed < payload_len {
        let CommonObjectHeader {
            object_type,
            object_length,
        } = read_common_object_header(&mut payload_cursor)?;
        let object_length = object_length as usize;

        if payload_consumed + 3 + object_length > payload_len {
            return Err(Box::new(IoError::new(
                ErrorKind::InvalidData,
                format!(
                    "object length exceeds payload (object length = {}, payload length = {}, consumed = {})",
                    object_length, payload_len, payload_consumed
                ),
            )));
        }
        payload_consumed += 3;

        match object_type {
            ProtocolObjectType::FetchRspData => {
                if object_length < 1 {
                    return Err(Box::new(IoError::new(
                        ErrorKind::InvalidData,
                        format!(
                            "fetch-rsp-data object too short (length = {}, expected at least 1)",
                            object_length
                        ),
                    )));
                }

                let status = u8::read_options(&mut payload_cursor, binrw::Endian::Big, ())?;

                match status {
                    FETCH_STATUS_NOT_FOUND => {
                        if object_length != 1 {
                            return Err(Box::new(IoError::new(
                                ErrorKind::InvalidData,
                                format!(
                                    "fetch-rsp not-found object must be exactly 1 byte (length = {})",
                                    object_length
                                ),
                            )));
                        }
                        return Ok(FetchRspData {
                            data: Err(FetchErr::NotFoundErr),
                        });
                    }
                    FETCH_STATUS_OK => {
                        if object_length < 3 {
                            return Err(Box::new(IoError::new(
                                ErrorKind::InvalidData,
                                format!(
                                    "fetch-rsp ok object too short (length = {}, expected at least 3)",
                                    object_length
                                ),
                            )));
                        }

                        let entry_count =
                            u16::read_options(&mut payload_cursor, binrw::Endian::Big, ())?
                                as usize;

                        let mut consumed_inside = 1 + 2;
                        let mut entries = Vec::with_capacity(entry_count);

                        for _ in 0..entry_count {
                            let entry_len =
                                u16::read_options(&mut payload_cursor, binrw::Endian::Big, ())?
                                    as usize;
                            consumed_inside += 2;

                            if consumed_inside + entry_len > object_length {
                                return Err(Box::new(IoError::new(
                                    ErrorKind::InvalidData,
                                    format!(
                                        "fetch-rsp entry exceeds object length (entry length = {}, object length = {})",
                                        entry_len, object_length
                                    ),
                                )));
                            }

                            let mut item = vec![0u8; entry_len];
                            payload_cursor.read_exact(&mut item)?;
                            consumed_inside += entry_len;
                            entries.push(Arc::from(item));
                        }

                        if consumed_inside != object_length {
                            return Err(Box::new(IoError::new(
                                ErrorKind::InvalidData,
                                format!(
                                    "fetch-rsp-data object has trailing bytes (length = {}, consumed = {})",
                                    object_length, consumed_inside
                                ),
                            )));
                        }

                        return Ok(FetchRspData { data: Ok(entries) });
                    }
                    other => {
                        return Err(Box::new(IoError::new(
                            ErrorKind::InvalidData,
                            format!("invalid fetch-rsp status: {other:#x}"),
                        )));
                    }
                }
            }
            _ => {
                payload_cursor.seek(SeekFrom::Current(object_length as i64))?;
            }
        }

        payload_consumed += object_length;
    }

    Err(Box::new(IoError::new(
        ErrorKind::InvalidData,
        format!(
            "missing fetch-rsp-data object (consumed = {})",
            payload_consumed
        ),
    )))
}

fn deserialize_uln_disc_rsp<R: Read>(
    header: CommonHeader,
    reader: &mut R,
) -> Result<ProtocolMessage, Box<dyn Error>> {
    let parsed = deserialize_req_rsp_payload(&header, reader)?;
    Ok(ProtocolMessage::ULNDiscRsp(ULNReqRspMessage {
        common_header: header,
        data: RTableData {
            contacts: parsed.contacts,
        },
    }))
}

fn deserialize_uln_disc_req<R: Read>(
    header: CommonHeader,
    reader: &mut R,
) -> Result<ProtocolMessage, Box<dyn Error>> {
    let parsed = deserialize_req_rsp_payload(&header, reader)?;
    Ok(ProtocolMessage::ULNDiscReq(ULNReqRspMessage {
        common_header: header,
        data: RTableData {
            contacts: parsed.contacts,
        },
    }))
}

fn deserialize_query_route_req<R: Read>(
    header: CommonHeader,
    reader: &mut R,
) -> Result<ProtocolMessage, Box<dyn Error>> {
    let parsed = deserialize_req_rsp_payload(&header, reader)?;
    let Some(source_route) = parsed.source_route else {
        return Err(Box::new(IoError::new(
            ErrorKind::InvalidData,
            "missing source-route object in QueryRouteReq",
        )));
    };

    let (request_type, _radius) = parsed.rtable_request.ok_or_else(|| {
        IoError::new(
            ErrorKind::InvalidData,
            "missing rtable-request object in QueryRouteReq",
        )
    })?;

    let query_type = match request_type {
        RTableRequestTypeValue::ULNVicinity => QueryRouteType::UnderlayNeighbors,
        _ => {
            return Err(Box::new(IoError::new(
                ErrorKind::InvalidData,
                "unsupported rtable-request for QueryRouteReq",
            )));
        }
    };

    Ok(ProtocolMessage::QueryRouteReq(ReqRspMessage {
        common_header: header,
        data: QueryRouteReqData { query_type },
        not_via: Option::from(parsed.not_via),
        source_route,
    }))
}

fn deserialize_query_route_rsp<R: Read>(
    header: CommonHeader,
    reader: &mut R,
) -> Result<ProtocolMessage, Box<dyn Error>> {
    let parsed = deserialize_req_rsp_payload(&header, reader)?;
    let Some(source_route) = parsed.source_route else {
        return Err(Box::new(IoError::new(
            ErrorKind::InvalidData,
            "missing source-route object in QueryRouteRsp",
        )));
    };

    Ok(ProtocolMessage::QueryRouteRsp(ReqRspMessage {
        common_header: header,
        data: RTableData {
            contacts: parsed.rtable,
        },
        not_via: Option::from(parsed.not_via),
        source_route,
    }))
}

fn deserialize_find_node_req<R: Read>(
    header: CommonHeader,
    reader: &mut R,
) -> Result<ProtocolMessage, Box<dyn Error>> {
    let parsed = deserialize_req_rsp_payload(&header, reader)?;
    let Some(source_route) = parsed.source_route else {
        return Err(Box::new(IoError::new(
            ErrorKind::InvalidData,
            "missing source-route object in FindNodeReq",
        )));
    };

    let (_request_type, radius) = parsed.rtable_request.ok_or_else(|| {
        IoError::new(
            ErrorKind::InvalidData,
            "missing rtable-request object in FindNodeReq",
        )
    })?;
    let neighborhood = NonZeroU64::new(u64::from(radius)).ok_or_else(|| {
        IoError::new(
            ErrorKind::InvalidData,
            "FindNodeReq radius in rtable-request must be > 0",
        )
    })?;

    Ok(ProtocolMessage::FindNodeReq(ReqRspMessage {
        common_header: header,
        data: FindNodeReqData { neighborhood },
        not_via: Option::from(parsed.not_via),
        source_route,
    }))
}

fn deserialize_find_node_rsp<R: Read>(
    header: CommonHeader,
    reader: &mut R,
) -> Result<ProtocolMessage, Box<dyn Error>> {
    let parsed = deserialize_req_rsp_payload(&header, reader)?;
    let Some(source_route) = parsed.source_route else {
        return Err(Box::new(IoError::new(
            ErrorKind::InvalidData,
            "missing source-route object in FindNodeRsp",
        )));
    };

    Ok(ProtocolMessage::FindNodeRsp(ReqRspMessage {
        common_header: header,
        data: RTableData {
            contacts: parsed.rtable,
        },
        not_via: Option::from(parsed.not_via),
        source_route,
    }))
}

fn deserialize_update_route_req<R: Read>(
    header: CommonHeader,
    reader: &mut R,
) -> Result<ProtocolMessage, Box<dyn Error>> {
    let payload = read_payload_bytes(&header, reader)?;
    let parsed = parse_req_rsp_payload_from_bytes(&header, &payload)?;
    let contact_actions = parse_rtable_update_info_from_bytes(&payload)?;
    let Some(source_route) = parsed.source_route else {
        return Err(Box::new(IoError::new(
            ErrorKind::InvalidData,
            "missing source-route object in UpdateRouteReq",
        )));
    };

    Ok(ProtocolMessage::UpdateRouteReq(UpdateRouteReq {
        common_header: header,
        not_via: Option::from(parsed.not_via),
        contact_actions,
        source_route,
    }))
}

#[derive(Debug, Default)]
struct ParsedReqRspPayload {
    source_route: Option<SourceRoute>,
    not_via: HashSet<NotVia>,
    contacts: Vec<Contact>,
    rtable: Vec<Contact>,
    rtable_request: Option<(RTableRequestTypeValue, u8)>,
}

fn deserialize_req_rsp_payload<R: Read>(
    header: &CommonHeader,
    reader: &mut R,
) -> Result<ParsedReqRspPayload, Box<dyn Error>> {
    let payload = read_payload_bytes(header, reader)?;
    parse_req_rsp_payload_from_bytes(header, &payload)
}

fn read_payload_bytes<R: Read>(header: &CommonHeader, reader: &mut R) -> Result<Vec<u8>, IoError> {
    let payload_len = (header.msg_length() - HEADER_LEN as u16) as usize;
    let mut payload = vec![0u8; payload_len];
    reader.read_exact(&mut payload)?;
    Ok(payload)
}

fn parse_req_rsp_payload_from_bytes(
    _header: &CommonHeader,
    payload: &[u8],
) -> Result<ParsedReqRspPayload, Box<dyn Error>> {
    let payload_len = payload.len();

    let mut payload_cursor = binrw::io::Cursor::new(payload);
    let mut payload_consumed = 0;

    let mut parsed = ParsedReqRspPayload::default();
    let ParsedReqRspPayload {
        source_route,
        not_via,
        contacts,
        rtable,
        rtable_request,
    } = &mut parsed;

    while payload_consumed < payload_len {
        let CommonObjectHeader {
            object_type,
            object_length,
        } = read_common_object_header(&mut payload_cursor)?;
        let object_length = object_length as usize;

        if payload_consumed + 3 + object_length > payload_len {
            return Err(Box::new(IoError::new(
                ErrorKind::InvalidData,
                format!("object length exceeds payload: {object_length}, {object_type:?}"),
            )));
        }

        payload_consumed += 3;

        // TODO: Refactor into separate methods or dedicated structs
        match object_type {
            ProtocolObjectType::SourceRoute => {
                if object_length < 2 {
                    return Err(Box::new(IoError::new(
                        ErrorKind::InvalidData,
                        "source route object too short",
                    )));
                }

                let index =
                    u16::read_options(&mut payload_cursor, binrw::Endian::Big, ())? as usize;
                payload_consumed += 2;

                let remaining = object_length - 2;
                if !remaining.is_multiple_of(NodeId::SIZE) {
                    return Err(Box::new(IoError::new(
                        ErrorKind::InvalidData,
                        "source route object has invalid length",
                    )));
                }

                let hop_count = remaining / NodeId::SIZE;
                if hop_count < 2 {
                    // Reject SourceRoutes with bogus lengths
                    return Err(Box::new(IoError::new(
                        ErrorKind::InvalidData,
                        "source route without hops",
                    )));
                }

                let mut hops = Vec::with_capacity(hop_count);
                for _ in 0..hop_count {
                    let mut nid_bytes = [0u8; NodeId::SIZE];
                    payload_cursor.read_exact(&mut nid_bytes)?;
                    hops.push(NodeId::from(nid_bytes));
                }
                payload_consumed += remaining;

                let path = hops
                    .clone()
                    .into_iter()
                    .collect::<Result<_, _>>()
                    .expect("hop count verified >= 2");
                let mut sr = SourceRoute::try_from(path).expect("hop count verified >= 2");

                for _ in 1..index {
                    sr.advance();
                }
                debug_assert_eq!(index, sr.traveled_hop_count());

                *source_route = Some(sr);
            }
            ProtocolObjectType::NotViaList => {
                if !object_length.is_multiple_of(32) {
                    return Err(Box::new(IoError::new(
                        ErrorKind::InvalidData,
                        "notvialist object has invalid length",
                    )));
                }

                let entries = object_length / 32;
                for _ in 0..entries {
                    use kira_r2kad::domain::Age;

                    let mut src = [0u8; NodeId::SIZE];
                    let mut dst = [0u8; NodeId::SIZE];
                    payload_cursor.read_exact(&mut src)?;
                    payload_cursor.read_exact(&mut dst)?;
                    let age_raw = u32::read_options(&mut payload_cursor, binrw::Endian::Big, ())?;
                    let link = Link::new(NodeId::from(src), NodeId::from(dst));
                    let age = Age::from(u64::from(age_raw));
                    let new_entry = NotVia::from((link, age));
                    not_via.insert(new_entry);
                }
                payload_consumed += object_length;
            }
            ProtocolObjectType::ContactList => {
                if !object_length.is_multiple_of(24) {
                    return Err(Box::new(IoError::new(
                        ErrorKind::InvalidData,
                        "contactlist object has invalid length",
                    )));
                }

                let entries = object_length / 24;
                for _ in 0..entries {
                    let mut id_bytes = [0u8; NodeId::SIZE];
                    payload_cursor.read_exact(&mut id_bytes)?;
                    let contact_id = NodeId::from(id_bytes);

                    let ssn_raw = u32::read_options(&mut payload_cursor, binrw::Endian::Big, ())?;
                    //TODO age and node degree?
                    let _age_raw = u32::read_options(&mut payload_cursor, binrw::Endian::Big, ())?; // consume placeholder
                    let _node_degree =
                        u16::read_options(&mut payload_cursor, binrw::Endian::Big, ())?;

                    let ssn = SafeStateSeqNr::try_from(ssn_raw).map_err(|_| {
                        IoError::new(
                            ErrorKind::InvalidData,
                            format!("invalid state_seq_num {ssn_raw}"),
                        )
                    })?;

                    let contact = Contact::new(Path::from(contact_id), ssn);
                    contacts.push(contact);
                }

                payload_consumed += object_length;
            }
            ProtocolObjectType::RTable => {
                let mut object_length_remaining = object_length;

                while object_length_remaining > 0 {
                    if object_length_remaining < 2 * NodeId::SIZE + 2 + 4 + 4 + 2 {
                        return Err(Box::new(IoError::new(
                            ErrorKind::InvalidData,
                            format!(
                                "rtable-entry object to short (length = {}, expected at least = {})",
                                object_length_remaining,
                                2 * NodeId::SIZE + 2 + 4 + 4 + 2
                            ),
                        )));
                    }

                    // Contact-ID
                    let mut id_bytes = [0u8; NodeId::SIZE];
                    payload_cursor.read_exact(&mut id_bytes)?;
                    let contact_id = NodeId::from(id_bytes);

                    // Path
                    let path_vector_size =
                        u16::read_options(&mut payload_cursor, binrw::Endian::Big, ())?;
                    if object_length_remaining
                        < NodeId::SIZE + path_vector_size as usize + 2 + 4 + 4 + 2
                    {
                        return Err(Box::new(IoError::new(
                            ErrorKind::InvalidData,
                            format!(
                                "rtable-entry object too short (length = {}, expected = {})",
                                object_length_remaining,
                                NodeId::SIZE + path_vector_size as usize + 2 + 4 + 4 + 2
                            ),
                        )));
                    }
                    let mut path = Vec::with_capacity(path_vector_size as usize);
                    for _ in 0..path_vector_size {
                        let mut id_bytes = [0u8; NodeId::SIZE];
                        payload_cursor.read_exact(&mut id_bytes)?;
                        let hop_id = NodeId::from(id_bytes);
                        path.push(hop_id);
                    }
                    let path = Path::try_from(path)?; // or domain model uses non-empty paths
                    assert_eq!(&contact_id, path.last()); // and does include the final destination

                    // SSN
                    let ssn_raw = u32::read_options(&mut payload_cursor, binrw::Endian::Big, ())?;
                    let ssn = SafeStateSeqNr::try_from(ssn_raw).map_err(|_| {
                        IoError::new(
                            ErrorKind::InvalidData,
                            format!("invalid state_seq_num {ssn_raw}"),
                        )
                    })?;

                    // TODO: Age
                    let _age_raw = u32::read_options(&mut payload_cursor, binrw::Endian::Big, ())?; // consume placeholder

                    // TODO: Node Degree
                    let _node_degree =
                        u16::read_options(&mut payload_cursor, binrw::Endian::Big, ())?;

                    let contact = Contact::new(path, ssn);
                    rtable.push(contact);

                    object_length_remaining -= NodeId::SIZE // Contact-ID
                        + (path_vector_size as usize * NodeId::SIZE) + 2 // Path
                        + 4 // SSN
                        + 4 // Age Info
                        + 2 // Node Degree;
                }

                payload_consumed += object_length;
            }
            ProtocolObjectType::RTableRequest => {
                if object_length != 2 {
                    return Err(Box::new(IoError::new(
                        ErrorKind::InvalidData,
                        "rtable-request object must be exactly 2 bytes",
                    )));
                }

                let req_type_raw = u8::read_options(&mut payload_cursor, binrw::Endian::Big, ())?;
                let radius = u8::read_options(&mut payload_cursor, binrw::Endian::Big, ())?;
                let req_type = RTableRequestTypeValue::from(req_type_raw);
                *rtable_request = Some((req_type, radius));
                payload_consumed += object_length;
            }
            ProtocolObjectType::RTableUpdateInfo
            | ProtocolObjectType::ErrorData
            | ProtocolObjectType::StoreReqData
            | ProtocolObjectType::StoreRspData
            | ProtocolObjectType::FetchReqData
            | ProtocolObjectType::FetchRspData
            | ProtocolObjectType::Other(_)
            | _ => {
                payload_cursor.seek(SeekFrom::Current(object_length as i64))?;
                payload_consumed += object_length;
            }
        }
    }

    Ok(parsed)
}

//helper: read a objectHeader
fn read_common_object_header<R: Read + Seek>(
    reader: &mut R,
) -> Result<CommonObjectHeader, IoError> {
    let object_type = ProtocolObjectType::from(
        u8::read_options(reader, binrw::Endian::Big, ())
            .map_err(|e| IoError::new(ErrorKind::InvalidData, e))?,
    );
    let object_length = u16::read_options(reader, binrw::Endian::Big, ())
        .map_err(|e| IoError::new(ErrorKind::InvalidData, e))?;
    Ok(CommonObjectHeader::new(object_type, object_length))
}

fn serialize_probe_req<W: Write>(
    writer: W,
    req: &ReqRspMessage<ProbeReqData>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    serialize_req_rsp_no_data(writer, req)
}

fn serialize_probe_rsp<W: Write>(
    writer: W,
    req: &ReqRspMessage<ProbeRspData>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    serialize_req_rsp_no_data(writer, req)
}

fn serialize_path_setup_req<W: Write>(
    writer: W,
    req: &ReqRspMessage<PathSetupReqData>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    serialize_req_rsp_no_data(writer, req)
}

fn serialize_path_teardown_req<W: Write>(
    writer: W,
    req: &ReqRspMessage<PathTeardownReqData>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    serialize_req_rsp_no_data(writer, req)
}

fn serialize_error<W: Write>(
    mut writer: W,
    req: &ReqRspMessage<ErrorData>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut payload = Vec::new();

    write_notvialist_object(&mut payload, &req.not_via)?;
    write_source_route_object(&mut payload, &req.source_route)?;
    write_error_data(&mut payload, &req.data)?;

    write_header_and_payload(&mut writer, &req.common_header, &payload)
}

fn write_error_data<W: Write>(writer: &mut W, data: &ErrorData) -> Result<usize, IoError> {
    match data {
        ErrorData::DeadEnd => {
            write_common_object_header(
                writer,
                CommonObjectHeader::new(ProtocolObjectType::ErrorData, 1),
            )?;
            writer.write_all(&[ERROR_DEAD_END])?;
            Ok(4)
        }
        ErrorData::SegmentFailure {
            failed_link,
            source,
        } => {
            let object_length = 1 + (NodeId::SIZE * 3);
            write_common_object_header(
                writer,
                CommonObjectHeader::new(ProtocolObjectType::ErrorData, (object_length) as u16),
            )?;

            writer.write_all(&[ERROR_SEGMENT_FAILURE])?;
            writer.write_all(&source.to_be_bytes())?;
            writer.write_all(&failed_link.first().to_be_bytes())?;
            writer.write_all(&failed_link.second().to_be_bytes())?;

            Ok(3 + object_length)
        }
    }
}

fn serialize_store_req<W: Write>(
    writer: W,
    req: &ReqRspMessage<StoreReqData<LHTInput>>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    serialize_req_rsp_with_data_obj(writer, req, write_store_req_data_object)
}

fn serialize_store_rsp<W: Write>(
    writer: W,
    req: &ReqRspMessage<StoreRspData>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    serialize_req_rsp_with_data_obj(writer, req, write_store_rsp_data_object)
}

fn serialize_fetch_req<W: Write>(
    writer: W,
    req: &ReqRspMessage<FetchReqData>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    serialize_req_rsp_with_data_obj(writer, req, write_fetch_req_data_object)
}

fn serialize_fetch_rsp<W: Write>(
    writer: W,
    req: &ReqRspMessage<FetchRspData<LHTOutput>>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    serialize_req_rsp_with_data_obj(writer, req, write_fetch_rsp_data_object)
}

fn serialize_req_rsp_no_data<W: Write, T: std::fmt::Debug>(
    mut writer: W,
    req: &ReqRspMessage<T>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut payload = Vec::new();

    write_source_route_object(&mut payload, &req.source_route)?;
    write_notvialist_object(&mut payload, &req.not_via)?;

    write_header_and_payload(&mut writer, &req.common_header, &payload)
}

fn serialize_req_rsp_with_data_obj<W: Write, T: std::fmt::Debug, F>(
    mut writer: W,
    req: &ReqRspMessage<T>,
    write_data_object: F,
) -> Result<(), Box<dyn Error + Send + Sync>>
where
    F: FnOnce(&mut Vec<u8>, &T) -> Result<usize, IoError>,
{
    let mut payload = Vec::new();

    write_source_route_object(&mut payload, &req.source_route)?;
    write_notvialist_object(&mut payload, &req.not_via)?;
    write_data_object(&mut payload, &req.data)?;

    write_header_and_payload(&mut writer, &req.common_header, &payload)
}

fn write_store_req_data_object<W: Write>(
    writer: &mut W,
    data: &StoreReqData<LHTInput>,
) -> Result<usize, IoError> {
    let data_len = data.data.len();
    if data_len > u16::MAX as usize {
        return Err(IoError::new(
            ErrorKind::InvalidInput,
            format!("store data too large: {data_len}"),
        ));
    }

    let age_len = data.last_accessed_ms.map_or(0, |_| 8);
    let object_length = NodeId::SIZE + 2 + data_len + age_len;
    write_common_object_header(
        writer,
        CommonObjectHeader::new(ProtocolObjectType::StoreReqData, object_length as u16),
    )?;

    writer.write_all(&data.handle.to_be_bytes())?;
    writer.write_all(&(data_len as u16).to_be_bytes())?;
    writer.write_all(&data.data)?;

    if let Some(age) = data.last_accessed_ms {
        let age_ms = u64::try_from(std::time::Duration::from(age).as_millis()).map_err(|_| {
            IoError::new(
                ErrorKind::InvalidInput,
                "last_accessed_ms overflowed u64 milliseconds",
            )
        })?;
        writer.write_all(&age_ms.to_be_bytes())?;
    }

    Ok(3 + object_length)
}

fn write_store_rsp_data_object<W: Write>(
    writer: &mut W,
    data: &StoreRspData,
) -> Result<usize, IoError> {
    write_common_object_header(
        writer,
        CommonObjectHeader::new(ProtocolObjectType::StoreRspData, 1),
    )?;

    let status = match &data.status {
        Ok(StoreOk::Created) => STORE_STATUS_CREATED,
        Ok(StoreOk::Inserted) => STORE_STATUS_INSERTED,
        Ok(StoreOk::Updated) => STORE_STATUS_UPDATED,
        Err(StoreErr::UnexpectedError(msg)) => {
            return Err(IoError::new(
                ErrorKind::InvalidData,
                format!("Cannot serialize StoreErr: {msg}"),
            ));
        }
    };
    writer.write_all(&[status])?;

    Ok(4)
}

fn write_fetch_req_data_object<W: Write>(
    writer: &mut W,
    data: &FetchReqData,
) -> Result<usize, IoError> {
    write_common_object_header(
        writer,
        CommonObjectHeader::new(ProtocolObjectType::FetchReqData, NodeId::SIZE as u16),
    )?;
    writer.write_all(&data.handle.to_be_bytes())?;
    Ok(3 + NodeId::SIZE)
}

fn write_fetch_rsp_data_object<W: Write>(
    writer: &mut W,
    data: &FetchRspData<LHTOutput>,
) -> Result<usize, IoError> {
    match &data.data {
        Ok(values) => {
            if values.len() > u16::MAX as usize {
                return Err(IoError::new(
                    ErrorKind::InvalidInput,
                    format!("too many fetch values: {}", values.len()),
                ));
            }

            let mut values_len = 0usize;
            for value in values {
                if value.len() > u16::MAX as usize {
                    return Err(IoError::new(
                        ErrorKind::InvalidInput,
                        format!("fetch value too large: {}", value.len()),
                    ));
                }
                values_len += 2 + value.len();
            }

            let object_length = 1 + 2 + values_len;
            if object_length > u16::MAX as usize {
                return Err(IoError::new(
                    ErrorKind::InvalidInput,
                    format!("fetch response too large: {object_length}"),
                ));
            }

            write_common_object_header(
                writer,
                CommonObjectHeader::new(ProtocolObjectType::FetchRspData, object_length as u16),
            )?;

            writer.write_all(&[FETCH_STATUS_OK])?;
            writer.write_all(&(values.len() as u16).to_be_bytes())?;
            for value in values {
                writer.write_all(&(value.len() as u16).to_be_bytes())?;
                writer.write_all(value)?;
            }

            Ok(3 + object_length)
        }
        Err(FetchErr::NotFoundErr) => {
            write_common_object_header(
                writer,
                CommonObjectHeader::new(ProtocolObjectType::FetchRspData, 1),
            )?;
            writer.write_all(&[FETCH_STATUS_NOT_FOUND])?;
            Ok(4)
        }
    }
}

fn write_header_and_payload<W: Write>(
    writer: &mut W,
    header: &CommonHeader,
    payload: &[u8],
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let total_len = HEADER_LEN
        .checked_add(payload.len())
        .ok_or_else(|| IoError::new(ErrorKind::InvalidData, "msg too large"))?;

    if total_len > u16::MAX as usize {
        return Err(Box::new(IoError::new(
            ErrorKind::InvalidData,
            "msg_length exceeds u16",
        )));
    }

    let mut header = header.clone();
    header.set_msg_length(total_len as u16);

    let mut cursor = binrw::io::Cursor::new(Vec::with_capacity(HEADER_LEN));
    header.write_options(&mut cursor, binrw::Endian::Big, ())?;
    writer.write_all(cursor.get_ref())?;
    writer.write_all(payload)?;
    Ok(())
}

fn serialize_query_route_req<W: Write>(
    writer: W,
    req: &ReqRspMessage<QueryRouteReqData>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let (request_type, radius) = match req.data.query_type {
        QueryRouteType::UnderlayNeighbors => (
            RTableRequestTypeValue::ULNVicinity,
            req.common_header.src_node_degree().min(u8::MAX as u16) as u8,
        ),
    };

    serialize_req_rsp_with_rtable_request(writer, req, request_type, radius)
}

fn serialize_find_node_req<W: Write>(
    writer: W,
    req: &ReqRspMessage<FindNodeReqData>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let radius = req.data.neighborhood.get().min(u8::MAX as u64) as u8;
    let request_type = RTableRequestTypeValue::OverlayNeighbors;

    serialize_req_rsp_with_rtable_request(writer, req, request_type, radius)
}

fn serialize_update_route_req<W: Write>(
    mut writer: W,
    req: &UpdateRouteReq,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut payload = Vec::new();

    write_source_route_object(&mut payload, &req.source_route)?;
    write_notvialist_object(&mut payload, &req.not_via)?;
    write_rtable_update_info_object(&mut payload, &req.contact_actions)?;

    write_header_and_payload(&mut writer, &req.common_header, &payload)
}

fn serialize_req_rsp_with_rtable_request<W: Write, T: std::fmt::Debug>(
    mut writer: W,
    req: &ReqRspMessage<T>,
    request_type: RTableRequestTypeValue,
    radius: u8,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut payload = Vec::new();

    write_rtable_request_object(&mut payload, request_type, radius)?;
    write_source_route_object(&mut payload, &req.source_route)?;
    write_notvialist_object(&mut payload, &req.not_via)?;

    write_header_and_payload(&mut writer, &req.common_header, &payload)
}

fn write_rtable_request_object<W: Write>(
    writer: &mut W,
    request_type: RTableRequestTypeValue,
    radius: u8,
) -> Result<usize, IoError> {
    write_common_object_header(
        writer,
        CommonObjectHeader::new(ProtocolObjectType::RTableRequest, 2),
    )?;

    let req: u8 = request_type.into();
    writer.write_all(&[req, radius])?;

    Ok(5)
}

fn serialize_uln_req_rsp_rtable<W: Write>(
    mut writer: W,
    req: &ULNReqRspMessage<RTableData>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut payload = Vec::new();

    write_contactlist_object(&mut payload, &req.data.contacts)?;

    write_header_and_payload(&mut writer, &req.common_header, &payload)
}

fn serialize_req_rsp_rtable<W: Write>(
    mut writer: W,
    req: &ReqRspMessage<RTableData>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut payload = Vec::new();

    write_source_route_object(&mut payload, &req.source_route)?;
    write_notvialist_object(&mut payload, &req.not_via)?;
    write_rtable_object(&mut payload, &req.data.contacts)?;

    write_header_and_payload(&mut writer, &req.common_header, &payload)
}

fn write_common_object_header<W: Write>(
    writer: &mut W,
    header: CommonObjectHeader,
) -> Result<(), IoError> {
    let value_obj_type: u8 = header.object_type.into();
    writer.write_all(&[value_obj_type])?;
    writer.write_all(&header.object_length.to_be_bytes())?;
    Ok(())
}

fn write_source_route_object<W: Write>(
    writer: &mut W,
    source_route: &SourceRoute,
) -> Result<usize, IoError> {
    let index = source_route_index(source_route)?;

    let object_length = size_of_val(&index) + source_route.size() * NodeId::SIZE;
    write_common_object_header(
        writer,
        CommonObjectHeader::new(ProtocolObjectType::SourceRoute, object_length as u16),
    )?;

    writer.write_all(&(index).to_be_bytes())?;
    for hop in source_route.iter() {
        writer.write_all(&hop.to_be_bytes())?;
    }

    Ok(3 + object_length)
}

fn source_route_index(source_route: &SourceRoute) -> Result<u16, IoError> {
    let idx = source_route.traveled_hop_count();
    if idx > 1023 {
        return Err(IoError::new(
            ErrorKind::InvalidInput,
            format!("source route index {idx} out of range"),
        ));
    }

    Ok(idx.try_into().expect("idx <= 1023"))
}

fn write_notvialist_object<W: Write>(
    writer: &mut W,
    not_via: &Option<HashSet<NotVia>>,
) -> Result<usize, IoError> {
    let mut links: Vec<Link> = Vec::new();
    if let Some(not_via_set) = not_via {
        for entry in not_via_set {
            let link = entry.link.clone();
            links.push(link);
        }
    }

    if links.is_empty() {
        return Ok(0);
    }

    let object_length = links.len() * (NodeId::SIZE * 2 + 4);
    write_common_object_header(
        writer,
        CommonObjectHeader::new(ProtocolObjectType::NotViaList, object_length as u16),
    )?;

    for link in links {
        writer.write_all(&link.first().to_be_bytes())?;
        writer.write_all(&link.second().to_be_bytes())?;
        writer.write_all(&0u32.to_be_bytes())?; // age-info placeholder
    }

    Ok(3 + object_length)
}

fn write_contactlist_object<W: Write>(
    writer: &mut W,
    contacts: &[Contact],
) -> Result<usize, IoError> {
    if contacts.is_empty() {
        return Ok(0);
    }

    if contacts.len() > u16::MAX as usize {
        return Err(IoError::new(
            ErrorKind::InvalidInput,
            format!("too many contacts: {}", contacts.len()),
        ));
    }

    let object_length: usize = contacts.len()
        * (
            NodeId::SIZE // Contact-ID
                + 4 // SSN
                + 4 // TODO: Age Info
                + 2
            // TODO: Node Degree
        );
    write_common_object_header(
        writer,
        CommonObjectHeader::new(ProtocolObjectType::ContactList, object_length as u16),
    )?;

    for contact in contacts {
        write_contact_entry(writer, contact)?;
    }

    Ok(3 + object_length)
}

fn write_contact_entry<W: Write>(writer: &mut W, contact: &Contact) -> Result<(), IoError> {
    // Contact-ID
    writer.write_all(&contact.id().to_be_bytes())?;

    // SSN
    let ssn: u32 = (*contact.state_seq_nr()).into();
    writer.write_all(&ssn.to_be_bytes())?;

    // Age Info
    writer.write_all(&u32::MAX.to_be_bytes())?; // TODO: Serialize age-info of Contact in rtable

    // Node Degree
    writer.write_all(&u16::MAX.to_be_bytes())?; // TODO: Serialize node degree of Contact in rtable

    Ok(())
}

fn write_rtable_object<W: Write>(writer: &mut W, contacts: &[Contact]) -> Result<usize, IoError> {
    if contacts.is_empty() {
        return Ok(0);
    }

    if contacts.len() > u16::MAX as usize {
        return Err(IoError::new(
            ErrorKind::InvalidInput,
            format!("too many contacts: {}", contacts.len()),
        ));
    }

    let object_length: usize = contacts
        .iter()
        .map(|c| {
            NodeId::SIZE // Contact-ID
                + c.path()
                    .map(Path::size).unwrap_or(0)
                    * NodeId::SIZE + 2 // Path-Vector
                + 4 // SSN
                + 4 // TODO: Age Info
                + 2 // TODO: Node Degree
        })
        .sum();
    write_common_object_header(
        writer,
        CommonObjectHeader::new(ProtocolObjectType::RTable, object_length as u16),
    )?;

    // FIXME: Write rtable-length (#contacts)

    for contact in contacts {
        write_rtable_entry(writer, contact)?;
    }

    Ok(3 + object_length)
}

fn write_rtable_entry<W: Write>(writer: &mut W, contact: &Contact) -> Result<(), IoError> {
    // Contact-ID
    writer.write_all(&contact.id().to_be_bytes())?;

    // Path
    let path_length = contact.path().map(Path::size).unwrap_or(0);
    writer.write_all(
        &u16::try_from(path_length)
            .map_err(|_| IoError::new(ErrorKind::InvalidData, "path-vector to long".to_string()))?
            .to_be_bytes(),
    )?;
    if let Some(path) = contact.path() {
        for hop in path.into_iter() {
            writer.write_all(&hop.to_be_bytes())?;
        }
    }

    // SSN
    let ssn: u32 = (*contact.state_seq_nr()).into();
    writer.write_all(&ssn.to_be_bytes())?;

    // Age Info
    writer.write_all(&u32::MAX.to_be_bytes())?; // TODO: Serialize age-info of Contact in rtable

    // Node Degree
    writer.write_all(&u16::MAX.to_be_bytes())?; // TODO: Serialize node degree of Contact in rtable

    Ok(())
}

fn parse_rtable_update_info_from_bytes(
    payload: &[u8],
) -> Result<HashMap<Contact, RouteUpdateActionType>, Box<dyn Error>> {
    let mut payload_cursor = binrw::io::Cursor::new(payload);
    let payload_len = payload.len();
    let mut payload_consumed = 0;

    while payload_consumed < payload_len {
        let CommonObjectHeader {
            object_type,
            object_length,
        } = read_common_object_header(&mut payload_cursor)?;
        let object_length = object_length as usize;

        if payload_consumed + 3 + object_length > payload_len {
            return Err(Box::new(IoError::new(
                ErrorKind::InvalidData,
                format!(
                    "object length exceeds payload (object length = {}, payload length = {}, consumed = {})",
                    object_length, payload_len, payload_consumed
                ),
            )));
        }
        payload_consumed += 3;

        if let ProtocolObjectType::RTableUpdateInfo = object_type {
            let mut object_length_remaining = object_length;
            let mut result: HashMap<Contact, RouteUpdateActionType> = HashMap::new();

            while object_length_remaining > 0 {
                if object_length_remaining < 2 * NodeId::SIZE + 2 + 4 + 4 + 2 + 1 {
                    return Err(Box::new(IoError::new(
                        ErrorKind::InvalidData,
                        format!(
                            "rtable-update-entry object too short (length = {}, expected at least {})",
                            object_length_remaining,
                            NodeId::SIZE + 5
                        ),
                    )));
                }

                // Contact-ID
                let mut id_bytes = [0u8; NodeId::SIZE];
                payload_cursor.read_exact(&mut id_bytes)?;
                let contact_id = NodeId::from(id_bytes);

                // Path
                let path_vector_size =
                    u16::read_options(&mut payload_cursor, binrw::Endian::Big, ())?;
                if object_length_remaining
                    < NodeId::SIZE + path_vector_size as usize + 2 + 4 + 4 + 2 + 1
                {
                    return Err(Box::new(IoError::new(
                        ErrorKind::InvalidData,
                        format!(
                            "rtable-update-entry object too short (length = {}, expected = {})",
                            object_length_remaining,
                            NodeId::SIZE + path_vector_size as usize + 2 + 4 + 4 + 2 + 1
                        ),
                    )));
                }
                let mut path = Vec::with_capacity(path_vector_size as usize);
                for _ in 0..path_vector_size {
                    let mut id_bytes = [0u8; NodeId::SIZE];
                    payload_cursor.read_exact(&mut id_bytes)?;
                    let hop_id = NodeId::from(id_bytes);
                    path.push(hop_id);
                }
                let path = Path::try_from(path)?; // or domain model uses non-empty paths
                assert_eq!(&contact_id, path.last()); // and does include the final destination

                // SSN
                let ssn_raw = u32::read_options(&mut payload_cursor, binrw::Endian::Big, ())?;
                let ssn = SafeStateSeqNr::try_from(ssn_raw).map_err(|_| {
                    IoError::new(
                        ErrorKind::InvalidData,
                        format!("invalid state_seq_num {ssn_raw}"),
                    )
                })?;

                // TODO: Age
                let _age_raw = u32::read_options(&mut payload_cursor, binrw::Endian::Big, ())?; // consume placeholder

                // TODO: Node Degree
                let _node_degree = u16::read_options(&mut payload_cursor, binrw::Endian::Big, ())?;

                // RouteUpdateAction
                let action_raw = u8::read_options(&mut payload_cursor, binrw::Endian::Big, ())?;
                let action = RouteUpdateActionType::from(action_raw);

                let contact = Contact::new(Path::from(contact_id), ssn);
                result.insert(contact, action);

                object_length_remaining -= NodeId::SIZE // Contact-ID
                        + (path_vector_size as usize * NodeId::SIZE)+ 2 // Path
                        + 4 // SSN
                        + 4 // Age Info
                        + 1 // RouteUpdateAction
                        + 2 // Node Degree;
            }

            return Ok(result);
        } else {
            payload_cursor.seek(SeekFrom::Current(object_length as i64))?;
            payload_consumed += object_length;
        }
    }

    // no rtable update info found
    Ok(HashMap::new())
}

fn write_rtable_update_info_object<W: Write>(
    writer: &mut W,
    data: &HashMap<Contact, RouteUpdateActionType>,
) -> Result<usize, IoError> {
    if data.is_empty() {
        return Ok(0);
    }

    if data.len() > u16::MAX as usize {
        return Err(IoError::new(
            ErrorKind::InvalidInput,
            format!("too many rtable update entries: {}", data.len()),
        ));
    }

    let object_length: usize = data
        .keys()
        .map(|c| {
            NodeId::SIZE // Contact-ID
                + c.path()
                    .map(Path::size).unwrap_or(0)
                    * NodeId::SIZE + 2 // Path-Vector
                + 4 // SSN
                + 4 // TODO: Age Info
                + 2 // TODO: Node Degree

                + 1 // RouteUpdateActionType
        })
        .sum();
    write_common_object_header(
        writer,
        CommonObjectHeader::new(ProtocolObjectType::RTableUpdateInfo, object_length as u16),
    )?;

    // FIXME: Write rtable-length (#contacts)

    for (contact, action) in data.iter() {
        write_rtable_update_entry(writer, contact, *action)?;
    }

    Ok(3 + object_length)
}

fn write_rtable_update_entry<W: Write>(
    writer: &mut W,
    contact: &Contact,
    action_type: RouteUpdateActionType,
) -> Result<(), IoError> {
    // Contact-ID
    writer.write_all(&contact.id().to_be_bytes())?;

    // Path
    let path_length = contact.path().map(Path::size).unwrap_or(0);
    writer.write_all(
        &u16::try_from(path_length)
            .map_err(|_| IoError::new(ErrorKind::InvalidData, "path-vector to long"))?
            .to_be_bytes(),
    )?;
    if let Some(path) = contact.path() {
        for hop in path.into_iter() {
            writer.write_all(&hop.to_be_bytes())?;
        }
    }

    // SSN
    let ssn: u32 = (*contact.state_seq_nr()).into();
    writer.write_all(&ssn.to_be_bytes())?;

    // Age Info
    writer.write_all(&u32::MAX.to_be_bytes())?; // TODO: Serialize age-info of Contact in rtable

    // Node Degree
    writer.write_all(&u16::MAX.to_be_bytes())?; // TODO: Serialize node degree of Contact in rtable

    // RouteUpdateActionType
    writer.write_all(&u8::from(action_type).to_be_bytes())?;

    Ok(())
}
