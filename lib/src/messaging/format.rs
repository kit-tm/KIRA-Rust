use std::error::Error;
use std::io::{Read, Write};

use crate::messaging::ProtocolMessage;
use serde::Serialize;

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
    ) -> Result<(), Box<dyn Error>> {
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
