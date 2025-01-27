//! Concrete serialization and deserialization implementation of
//! [ProtocolMessages](ProtocolMessage) on a closed set of supported formats.

use std::error::Error;
use std::io::{Read, Write};

use serde::Serialize;

use kira_lib::messaging::ProtocolMessage;

/// Implementation of the interface ProtocolMessageFormat as closed set of
/// supported formats.
///
/// Instead of using a trait this implementation supports easy to use methods for a closed set of transmission formats.
/// As it's currently not desired to support a broad set of transmission formats this decision has been made.
// TODO: Refactor this to be more efficient. Currently it doesn't support proper buffer writing.
#[derive(Debug, Copy, Clone)]
pub enum ProtocolMessageFormat {
    #[cfg(feature = "serde_json")]
    /// [JavaScript object notation](https://www.json.org) message format
    Json,
    #[cfg(feature = "rmp-serde")]
    /// [MessagePack](https://msgpack.org/) message format.
    ///
    /// MessagePack is similar to [Json](Self::Json) but more compact and
    /// should be preferred unless readability is a concern.
    MessagePack,
    /// No message format enabled.
    ///
    /// This usually results in a panic if trying to [serialize](Self::serialize) or
    /// [deserialize](Self::deserialize) [ProtocolMessages](ProtocolMessages).
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
            #[cfg(feature = "serde_json")]
            Self::Json => serde_json::from_reader(reader)?,
            #[cfg(feature = "rmp-serde")]
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
            #[cfg(feature = "serde_json")]
            Self::Json => serde_json::to_writer(writer, data)?,
            #[cfg(feature = "rmp-serde")]
            Self::MessagePack => data.serialize(&mut rmp_serde::Serializer::new(writer))?,
            Self::None => panic!("No Format enabled"),
        };

        Ok(())
    }
}
