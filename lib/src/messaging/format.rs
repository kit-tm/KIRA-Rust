use std::error::Error;
use std::io::{Read, Write};

use serde::Serialize;

use crate::messaging::ProtocolMessage;

/// Implementation of the interface ProtocolMessageFormat as closed set of
/// supported formats.
///
/// Instead of using a trait this implementation supports easy to use methods for a closed set of transmission formats.
/// As it's currently not desired to support a broad set of transmission formats this decision has been made.
// TODO: Refactor this to be more efficient. Currently it doesn't support proper buffer writing.
#[derive(Debug, Clone)]
pub enum ProtocolMessageFormat {
    #[cfg(feature = "serde_json")]
    Json,
    #[cfg(feature = "rmp-serde")]
    MessagePack,
    None,
}

impl ProtocolMessageFormat {
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
