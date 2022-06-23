use std::error::Error;
use std::io::{Read, Write};

use serde::de::DeserializeOwned;
use serde::Serialize;

#[derive(Debug, Clone)]
pub enum Format {
    #[cfg(feature = "serde_json")]
    Json,
    #[cfg(feature = "rmp-serde")]
    MessagePack,
    None,
}

impl Format {
    pub fn deserialize<R: Read, T: DeserializeOwned>(
        &self,
        reader: R,
    ) -> Result<T, Box<dyn Error>> {
        let result = match self {
            #[cfg(feature = "serde_json")]
            Self::Json => serde_json::from_reader(reader)?,
            #[cfg(feature = "rmp-serde")]
            Self::MessagePack => rmp_serde::from_read(reader)?,
            Self::None => panic!("No Format enabled"),
        };

        Ok(result)
    }

    pub fn serialize<W: Write, T: Serialize>(
        &self,
        writer: W,
        data: &T,
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
