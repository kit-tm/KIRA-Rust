//! Concrete serialization and deserialization implementation of
//! [ProtocolMessages](ProtocolMessage) on a closed set of supported formats.

use std::error::Error;
use std::io::{Error as IoError, ErrorKind, Read, Write};

#[cfg(feature = "format-binrw")]
use binrw::{BinRead, BinWrite, binrw};
use kira_r2kad::messaging::ProtocolMessage;
#[cfg(feature = "format-binrw")]
use kira_r2kad::messaging::{CommonHeader, ProtocolMessageKind};
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
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn binrw_ulnhello() {
        let mut header = CommonHeader::new(
            ProtocolMessageKind::ULNHello,
            kira_r2kad::domain::NodeId::with_lsb(0x12),
            kira_r2kad::domain::NodeId::with_lsb(0x34),
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

        match decoded {
            ProtocolMessage::ULNHello(h) => {
                assert_eq!(h.dest_id(), header.dest_id());
                assert_eq!(h.src_node_id(), header.src_node_id());
                assert_eq!(h.domain_id(), header.domain_id());
                assert_eq!(h.msg_id(), header.msg_id());
                assert_eq!(h.state_seq_num(), header.state_seq_num());
                assert_eq!(h.src_node_degree(), header.src_node_degree());
                assert_eq!(h.msg_length(), header.msg_length());
                assert_eq!(h.msg_type(), header.msg_type());
            }
            other => panic!("unexpected message: {other:?}"),
        }
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
    const HEADER_LEN: usize = 55; // KIRA header size (bytes)

    let mut buf = [0u8; HEADER_LEN];
    reader.read_exact(&mut buf)?;

    let mut cursor = binrw::io::Cursor::new(&buf);
    let header = CommonHeader::read_options(&mut cursor, binrw::Endian::Big, ())?;
    if header.msg_type() != ProtocolMessageKind::ULNHello as u8 {
        return Err(Box::new(IoError::new(
            ErrorKind::Unsupported,
            format!(
                "binrw currently supports ULNHello only (got msg_type {:#x})",
                header.msg_type()
            ),
        )));
    }

    Ok(ProtocolMessage::ULNHello(header))
}

#[cfg(feature = "format-binrw")]
fn serialize_binrw<W: Write>(
    mut writer: W,
    message: &ProtocolMessage,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    match message {
        ProtocolMessage::ULNHello(header) => {
            let mut cursor = binrw::io::Cursor::new(Vec::with_capacity(55));
            header.write_options(&mut cursor, binrw::Endian::Big, ())?;
            writer.write_all(cursor.get_ref())?;
            Ok(())
        }
        _ => Err(Box::new(IoError::new(
            ErrorKind::Unsupported,
            "binrw currently only supports ULNHello",
        ))),
    }
}
