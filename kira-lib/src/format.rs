//! Concrete serialization and deserialization implementation of
//! [ProtocolMessages](ProtocolMessage) on a closed set of supported formats.

use std::error::Error;
use std::io::{Read, Write};

use kira_r2kad::messaging::ProtocolMessage;
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

impl Default for ProtocolMessageFormat {
    /// Defaults to [Self::None].
    ///
    /// You must explicitly enable a [ProtocolMessageFormat] if wanted.
    fn default() -> Self {
        Self::None
    }
}

impl ProtocolMessageFormat {
    /// Deserializes a [ProtocolMessage] from a [Reader](Read).
    ///
    /// If no message format was selected this method panics.
    pub fn deserialize<R: Read>(&self, reader: R) -> Result<ProtocolMessage, Box<dyn Error>> {
        let result = match self {
            #[cfg(feature = "format-cbor")]
            Self::CBOR => ciborium::from_reader(reader)?,
            #[cfg(feature = "format-json")]
            Self::Json => serde_json::from_reader(reader)?,
            #[cfg(feature = "format-mp")]
            Self::MessagePack => rmp_serde::from_read(reader)?,
            Self::None => panic!("No Format enabled"),
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
            #[cfg(feature = "format-cbor")]
            Self::CBOR => ciborium::into_writer(data, writer)?,
            #[cfg(feature = "format-json")]
            Self::Json => serde_json::to_writer(writer, data)?,
            #[cfg(feature = "format-mp")]
            Self::MessagePack => data.serialize(&mut rmp_serde::Serializer::new(writer))?,
            Self::None => panic!("No Format enabled"),
        };

        Ok(())
    }
}
