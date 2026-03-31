//! Concrete serialization and deserialization implementation of
//! [ProtocolMessages](ProtocolMessage) on a closed set of supported formats.

use std::collections::HashSet;
use std::error::Error;
use std::io::{Error as IoError, ErrorKind, Read, Seek, SeekFrom, Write};

#[cfg(feature = "format-binrw")]
const HEADER_LEN: usize = 55; // KIRA header size (bytes)

use binrw::{self, BinRead, BinWrite};
use kira_r2kad::domain::{Contact, Link, NodeId, NotVia, Path, SafeStateSeqNr};
use kira_r2kad::messaging::source_route::SourceRoute;
use kira_r2kad::messaging::{
    CommonHeader, CommonObjectHeader, ProtocolMessage, ProtocolObjectType, RTableData,
    ReqRspMessage,
};
#[cfg(any(
    feature = "format-json",
    feature = "format-mp",
    feature = "format-cbor"
))]
use serde::Serialize;

/// Implementation of the interface ProtocolMessageFormat as closed set of
/// supported formats.
///
/// Instead of using a trait this implementation supports easy to use methods for a closed set of transmission formats.
/// As it's currently not desired to support a broad set of transmission formats this decision has been made.
// TODO: Refactor this to be more efficient. Currently it doesn't support proper buffer writing.
#[derive(Debug, Copy, Clone)]
pub enum ProtocolMessageFormat {
    #[cfg(feature = "format-binrw")]
    /// Binary serialization using the `binrw` format.
    BINRW,

    #[cfg(feature = "format-json")]
    /// [JavaScript object notation](https://www.json.org) message format
    Json,
    #[cfg(feature = "format-cbor")]
    /// [Concise Binary Object Representation (CBOR)](https://datatracker.ietf.org/doc/html/rfc8949) message format.
    ///
    /// CBOR is very efficient and a platform independent encoding, esp. used in IOT contexts
    /// This is the default encoding proposed by the KIRA specification
    CBOR,
    #[cfg(feature = "format-mp")]
    /// [MessagePack](https://msgpack.org/) message format.
    ///
    /// MessagePack is similar to JSON but more compact and
    /// should be preferred unless readability is a concern.
    MessagePack,
    /// No message format enabled.
    ///
    /// This usually results in a panic if trying to [serialize](Self::serialize) or
    /// [deserialize](Self::deserialize) [ProtocolMessages](ProtocolMessage).
    // WARNING: Why does this variant exist?
    None,
}

#[cfg(all(test, feature = "format-binrw"))]
mod binrw_tests {
    use super::*;
    use kira_r2kad::domain::{Contact, NodeId, Path, SafeStateSeqNr};
    use kira_r2kad::messaging::{ProtocolMessageKind, RTableData, ReqRspMessage};
    use std::collections::HashSet;
    use std::io::Cursor;

    #[test]
    fn binrw_hello() {
        let mut header = CommonHeader::new(
            ProtocolMessageKind::ULNHello,
            NodeId::with_lsb(0x12),
            NodeId::with_lsb(0x34),
            Some(0x789),
            Some(0x1234),
            1,
        );
        header.set_domain_id(0x4242);

        let msg = ProtocolMessage::ULNHello(header.clone());

        let mut buf = Vec::new();
        ProtocolMessageFormat::BINRW
            .serialize(&mut buf, &msg)
            .expect("serialize");

        let decoded = ProtocolMessageFormat::BINRW
            .deserialize(Cursor::new(&buf))
            .expect("deserialize");

        println!("Serialized into:");

        for (_i, byte) in buf.iter().enumerate() {
            print!("{:02x} ", byte);
        }
        println!();

        assert_eq!(decoded, ProtocolMessage::ULNHello(header));
    }

    #[test]
    fn binrw_discreq() {
        let mut header = CommonHeader::new(
            ProtocolMessageKind::ULNDiscReq,
            NodeId::with_lsb(0x10),
            NodeId::with_lsb(0x20),
            Some(0x1111),
            Some(0x2222),
            2,
        );
        header.set_domain_id(0x4242);

        let contact_id = NodeId::with_lsb(0x30);
        let ssn = SafeStateSeqNr::try_from(1u32).unwrap();
        let contact = Contact::new(Path::from(contact_id), ssn);

        let req = ReqRspMessage {
            common_header: header.clone(),
            data: RTableData {
                contacts: vec![contact.clone()],
            },
            not_via: HashSet::new(),
            source_route: SourceRoute::new(*header.src_node_id(), Path::from(*header.dest_id())),
        };

        let msg = ProtocolMessage::ULNDiscReq(req);

        let mut buf = Vec::new();
        ProtocolMessageFormat::BINRW
            .serialize(&mut buf, &msg)
            .expect("serialize");

        let decoded = ProtocolMessageFormat::BINRW
            .deserialize(Cursor::new(&buf))
            .expect("deserialize");

        println!("Serialized ULNDiscReq:");
        for byte in &buf {
            print!("{:02x} ", byte);
        }
        println!();

        let src_node_id = header.src_node_id();
        let dest_node_id = header.dest_id();

        let ProtocolMessage::ULNDiscReq(decoded_req) = &decoded else {
            panic!("unexpected message: {decoded:?}");
        };

        let hdr = &decoded_req.common_header;
        assert_eq!(hdr.msg_type(), header.msg_type());
        assert_eq!(hdr.dest_id(), header.dest_id());
        assert_eq!(hdr.src_node_id(), header.src_node_id());
        assert_eq!(hdr.domain_id(), header.domain_id());
        assert_eq!(hdr.msg_id(), header.msg_id());
        assert_eq!(hdr.state_seq_num(), header.state_seq_num());
        assert_eq!(hdr.src_node_degree(), header.src_node_degree());
        assert!(hdr.msg_length() >= HEADER_LEN as u16);

        assert!(decoded_req.not_via.is_empty());

        let got = &decoded_req.data.contacts[0];
        assert_eq!(got.id(), contact.id());
        assert_eq!(got.state_seq_nr(), contact.state_seq_nr());

        assert_eq!(decoded_req.source_route.source(), src_node_id);
        assert_eq!(decoded_req.source_route.destination(), dest_node_id);
    }
}

impl Default for ProtocolMessageFormat {
    /// Defaults to [Self::CBOR].
    ///
    /// You must explicitly enable a [ProtocolMessageFormat] if wanted.
    fn default() -> Self {
        #[cfg(feature = "format-cbor")]
        Self::CBOR
    }
}

impl ProtocolMessageFormat {
    /// Deserializes a [ProtocolMessage] from a [Reader](Read).
    ///
    /// If no message format was selected this method panics.
    pub fn deserialize<R: Read>(&self, reader: R) -> Result<ProtocolMessage, Box<dyn Error>> {
        let result = match self {
            #[cfg(feature = "format-binrw")]
            Self::BINRW => deserialize_binrw(reader)?,
            #[cfg(feature = "format-cbor")]
            Self::CBOR => serde_cbor::from_reader(reader)?,
            #[cfg(feature = "format-json")]
            Self::Json => serde_json::from_reader(reader)?,
            #[cfg(feature = "format-mp")]
            Self::MessagePack => rmp_serde::from_read(reader)?,
            Self::None => panic!("No PDU encoding format enabled"),
        };

        Ok(result)
    }

    /// Serializes a [ProtocolMessage] from using a [Writer](Write).
    ///
    /// If no message format was selected this method panics.
    pub fn serialize<W: Write>(
        &self,
        writer: W,
        data: &ProtocolMessage,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        match self {
            #[cfg(feature = "format-binrw")]
            Self::BINRW => serialize_binrw(writer, data)?,
            #[cfg(feature = "format-cbor")]
            Self::CBOR => {
                data.serialize(
                    &mut serde_cbor::Serializer::new(&mut serde_cbor::ser::IoWrite::new(writer))
                        .packed_format(),
                )?;
            }
            #[cfg(feature = "format-json")]
            Self::Json => serde_json::to_writer(writer, data)?,
            #[cfg(feature = "format-mp")]
            Self::MessagePack => data.serialize(&mut rmp_serde::Serializer::new(writer))?,
            Self::None => panic!("No PDU encoding format enabled"),
        };

        Ok(())
    }
}

#[cfg(feature = "format-binrw")]
fn deserialize_binrw<R: Read>(mut reader: R) -> Result<ProtocolMessage, Box<dyn Error>> {
    let mut buf = [0u8; HEADER_LEN];
    reader.read_exact(&mut buf)?;

    let mut cursor = binrw::io::Cursor::new(&buf);
    let header = CommonHeader::read_options(&mut cursor, binrw::Endian::Big, ())?;
    match header.msg_type() {
        0x01 => Ok(ProtocolMessage::ULNHello(header)),
        0x03 => {
            let payload_len = (header.msg_length() - HEADER_LEN as u16) as usize;
            let mut payload = vec![0u8; payload_len];
            reader.read_exact(&mut payload)?;

            let mut payload_cursor = binrw::io::Cursor::new(&payload);
            let mut payload_consumed = 0;

            let mut source_route: Option<SourceRoute> = None;
            let mut not_via: HashSet<NotVia> = HashSet::new();
            let mut contacts: Vec<Contact> = Vec::new();

            while payload_consumed < payload_len {
                let CommonObjectHeader {
                    object_type,
                    object_length,
                } = read_common_object_header(&mut payload_cursor)?;
                let object_length = object_length as usize;

                if payload_consumed + 3 + object_length > payload_len {
                    return Err(Box::new(IoError::new(
                        ErrorKind::InvalidData,
                        "object length exceeds payload",
                    )));
                }

                payload_consumed += 3;

                match object_type {
                    ProtocolObjectType::SourceRoute => {
                        if object_length < 2 {
                            return Err(Box::new(IoError::new(
                                ErrorKind::InvalidData,
                                "source route object too short",
                            )));
                        }

                        let index = u16::read_options(&mut payload_cursor, binrw::Endian::Big, ())?
                            as usize;
                        payload_consumed += 2;

                        let remaining = object_length - 2;
                        if remaining % NodeId::SIZE != 0 {
                            return Err(Box::new(IoError::new(
                                ErrorKind::InvalidData,
                                "source route object has invalid length",
                            )));
                        }

                        let hop_count = remaining / NodeId::SIZE;
                        if hop_count == 0 {
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

                        let path: Result<Path, _> = hops.clone().into_iter().collect();
                        let mut sr = SourceRoute::from(path.map_err(|err| {
                            IoError::new(ErrorKind::InvalidData, format!("{err}"))
                        })?);

                        for _ in 1..index {
                            sr.advance();
                        }

                        source_route = Some(sr);
                    }
                    ProtocolObjectType::NotViaList => {
                        if object_length % 32 != 0 {
                            return Err(Box::new(IoError::new(
                                ErrorKind::InvalidData,
                                "notvialist object has invalid length",
                            )));
                        }

                        let entries = object_length / 32;
                        for _ in 0..entries {
                            let mut src = [0u8; NodeId::SIZE];
                            let mut dst = [0u8; NodeId::SIZE];
                            payload_cursor.read_exact(&mut src)?;
                            payload_cursor.read_exact(&mut dst)?;
                            let _age =
                                u32::read_options(&mut payload_cursor, binrw::Endian::Big, ())?;
                            not_via.insert(NotVia::Link(Link::new(
                                NodeId::from(src),
                                NodeId::from(dst),
                            )));
                        }
                        payload_consumed += object_length;
                    }
                    ProtocolObjectType::ContactList => {
                        if object_length % 24 != 0 {
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

                            let ssn_raw =
                                u32::read_options(&mut payload_cursor, binrw::Endian::Big, ())?;
                            let _age =
                                u32::read_options(&mut payload_cursor, binrw::Endian::Big, ())?;
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
                    //other Objecttypes are ignored because they arent needed in DiscoveryReq
                    //TODO: Maybe return Error instead?
                    ProtocolObjectType::RTableRequest
                    | ProtocolObjectType::RTable
                    | ProtocolObjectType::RTableUpdateInfo
                    | ProtocolObjectType::Unknown(_) => {
                        payload_cursor.seek(SeekFrom::Current(object_length as i64))?;
                        payload_consumed += object_length;
                    }
                }
            }

            let source_route = source_route.unwrap_or_else(|| {
                SourceRoute::new(*header.src_node_id(), Path::from(*header.dest_id()))
            });

            let message = ReqRspMessage {
                common_header: header,
                data: RTableData { contacts },
                not_via,
                source_route,
            };

            Ok(ProtocolMessage::ULNDiscReq(message))
        }
        other => Err(Box::new(IoError::new(
            ErrorKind::Unsupported,
            format!("msg_type {:#x}, currently not supported by binrw", other),
        ))),
    }
}

//helper: read a objectHeader
#[cfg(feature = "format-binrw")]
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

#[cfg(feature = "format-binrw")]
fn serialize_binrw<W: Write>(
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
        ProtocolMessage::ULNDiscReq(req) => serialize_req_rsp_rtable(writer, req),
        ProtocolMessage::ULNDiscRsp(rsp) => serialize_req_rsp_rtable(writer, rsp),
        //todo: implement other message types
        //todo fix QueryRouteRsp and FindNodeRsp return
        ProtocolMessage::QueryRouteRsp(rsp) => serialize_req_rsp_rtable(writer, rsp),
        ProtocolMessage::FindNodeRsp(rsp) => serialize_req_rsp_rtable(writer, rsp),
        _ => Err(Box::new(IoError::new(
            ErrorKind::Unsupported,
            "currently not supported by binrw ",
        ))),
    }
}

#[cfg(feature = "format-binrw")]
fn serialize_req_rsp_rtable<W: Write>(
    mut writer: W,
    req: &ReqRspMessage<RTableData>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    let mut payload = Vec::new();

    let source_route_len = write_source_route_object(&mut payload, &req.source_route)?;
    let not_via_len = write_notvialist_object(&mut payload, &req.not_via)?;
    let contact_list_len = write_contactlist_object(&mut payload, &req.data.contacts)?;

    let payload_len = source_route_len + not_via_len + contact_list_len;
    let total_len = HEADER_LEN
        .checked_add(payload_len)
        .ok_or_else(|| IoError::new(ErrorKind::InvalidData, "msg too large"))?;

    if total_len > u16::MAX as usize {
        return Err(Box::new(IoError::new(
            ErrorKind::InvalidData,
            "msg_length exceeds u16",
        )));
    }

    let mut header = req.common_header.clone();
    header.set_msg_length(total_len as u16);

    let mut cursor = binrw::io::Cursor::new(Vec::with_capacity(HEADER_LEN));
    header.write_options(&mut cursor, binrw::Endian::Big, ())?;
    writer.write_all(cursor.get_ref())?;
    writer.write_all(&payload)?;
    Ok(())
}

#[cfg(feature = "format-binrw")]
fn write_common_object_header<W: Write>(
    writer: &mut W,
    header: CommonObjectHeader,
) -> Result<(), IoError> {
    let value_obj_type: u8 = header.object_type.into();
    writer.write_all(&[value_obj_type])?;
    writer.write_all(&header.object_length.to_be_bytes())?;
    Ok(())
}

#[cfg(feature = "format-binrw")]
fn write_source_route_object<W: Write>(
    writer: &mut W,
    source_route: &SourceRoute,
) -> Result<usize, IoError> {
    let route = collect_route_nodes(source_route)?;
    if route.is_empty() {
        return Err(IoError::new(
            ErrorKind::InvalidInput,
            "source route must contain at least one hop",
        ));
    }

    let index = source_route_index(source_route)?;
    let object_length = 2 + route.len() * NodeId::SIZE;
    write_common_object_header(
        writer,
        CommonObjectHeader::new(ProtocolObjectType::SourceRoute, object_length as u16),
    )?;

    writer.write_all(&(index as u16).to_be_bytes())?;
    for hop in route {
        writer.write_all(&hop.to_be_bytes())?;
    }

    Ok(3 + object_length)
}

#[cfg(feature = "format-binrw")]
fn source_route_index(source_route: &SourceRoute) -> Result<usize, IoError> {
    let size = source_route.size();
    let idx = if size == 1 {
        0
    } else {
        source_route.traveled_hop_count()
    };

    if idx > 1023 {
        return Err(IoError::new(
            ErrorKind::InvalidInput,
            format!("source route index {idx} out of range"),
        ));
    }

    Ok(idx)
}

#[cfg(feature = "format-binrw")]
fn collect_route_nodes(source_route: &SourceRoute) -> Result<Vec<NodeId>, IoError> {
    if source_route.size() == 1 {
        return Ok(vec![*source_route.source()]);
    }

    let traveled: Vec<NodeId> = Vec::from(source_route.traveled_path());
    let remaining: Vec<NodeId> = Vec::from(source_route.remaining_path());

    let mut route = Vec::with_capacity(traveled.len() + remaining.len());
    route.extend(traveled);
    route.extend(remaining);
    Ok(route)
}

#[cfg(feature = "format-binrw")]
fn write_notvialist_object<W: Write>(
    writer: &mut W,
    not_via: &HashSet<NotVia>,
) -> Result<usize, IoError> {
    let mut links: Vec<Link> = Vec::new();
    for entry in not_via {
        let NotVia::Link(link) = entry;
        links.push(link.clone());
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

#[cfg(feature = "format-binrw")]
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

    let object_length = contacts.len() * (NodeId::SIZE + 4 + 4 + 2);
    write_common_object_header(
        writer,
        CommonObjectHeader::new(ProtocolObjectType::ContactList, object_length as u16),
    )?;

    for contact in contacts {
        writer.write_all(&contact.id().to_be_bytes())?;

        let ssn: u32 = (*contact.state_seq_nr()).into();
        writer.write_all(&ssn.to_be_bytes())?;

        writer.write_all(&0u32.to_be_bytes())?; // age-info placeholder
        writer.write_all(&0u16.to_be_bytes())?; // node-degree placeholder
    }

    Ok(3 + object_length)
}
