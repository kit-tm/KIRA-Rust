use std::fmt::{Display, Formatter};
use axum::response::{IntoResponse, Response};
use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use hex::FromHexError;
use r2kad_lib::messaging::dht::{DefaultLHTInput, DefaultLHTOutput, FetchErr, StoreErr};
use r2kad_lib::use_cases::{FetchInjectData, StoreInjectData};
use serde_derive::Serialize;
use std::time::Duration;
use axum::http;
use r2kad_lib::domain::SIZE;
use sha2::{Digest, Sha256};
use crate::api::domain::NodeId;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

pub enum Handle {
    Handle(NodeId),
    Reference(String),
}

impl TryFrom<Handle> for r2kad_lib::domain::NodeId {
    type Error = FromHexError;

    fn try_from(value: Handle) -> Result<Self, Self::Error> {
        match value {
            Handle::Handle(handle) => handle.try_into(),
            Handle::Reference(reference) => {
                // calculating SHA256 hash
                let hash = Sha256::digest(reference.as_bytes());
                let handle: [u8; SIZE] = hash.into_iter()
                    .take(SIZE)
                    .collect::<Vec<u8>>()
                    .try_into()
                    .map_err(|_| {
                        log::error!(
                            target: "API",
                            "Length of hash result is to small to create NodeId: {} < {}",
                            hash.len(),
                            SIZE,
                        );
                        FromHexError::InvalidStringLength
                    })?;
                Ok(Self::from(handle))
            }
        }
    }
}

pub struct StoreArgs {
    pub handle: Handle,
    pub restore: bool,
    pub data: DefaultLHTInput,
}

impl From<StoreInjectData<DefaultLHTInput>> for StoreArgs {
    fn from(value: StoreInjectData<DefaultLHTInput>) -> Self {
        let handle = NodeId::from(value.handle);
        Self {
            handle: Handle::Handle(handle),
            restore: value.restore,
            data: value.data,
        }
    }
}

impl TryFrom<StoreArgs> for StoreInjectData<DefaultLHTInput>
{
    type Error = FromHexError;

    fn try_from(value: StoreArgs) -> Result<Self, Self::Error> {
        let handle = Handle::try_into(value.handle)?;
        Ok(Self {
            handle,
            data: value.data,
            restore: value.restore,
        })
    }
}

pub struct FetchArgs {
    pub handle: Handle,
}

impl TryFrom<FetchArgs> for FetchInjectData {
    type Error = FromHexError;

    fn try_from(value: FetchArgs) -> Result<Self, Self::Error> {
        let handle = value.handle.try_into()?;
        Ok(Self { handle })
    }
}

pub enum StoreOK {
    Created,
    Updated,
    Inserted,
}

impl Display for StoreOK {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Created => write!(f, "Created hashtable entry."),
            Self::Updated => write!(f, "Updated existing hashtable entry."),
            Self::Inserted => write!(f, "Inserted data into an existing hashtable entry."),
        }
    }
}

impl From<r2kad_lib::messaging::dht::StoreOK> for StoreOK {
    fn from(value: r2kad_lib::messaging::dht::StoreOK) -> Self {
        match value {
            r2kad_lib::messaging::dht::StoreOK::Created => Self::Created,
            r2kad_lib::messaging::dht::StoreOK::Updated => Self::Updated,
            r2kad_lib::messaging::dht::StoreOK::Inserted => Self::Inserted
        }
    }
}

impl From<StoreErr> for DHTErr {
    fn from(_value: StoreErr) -> Self {
        Self::Miscellaneous
    }
}

impl IntoResponse for StoreOK {
    fn into_response(self) -> Response {
        let status = match self {
            StoreOK::Created |
            StoreOK::Inserted => http::StatusCode::CREATED,
            StoreOK::Updated => http::StatusCode::OK,
        };

        (status, self.to_string()).into_response()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FetchRsp(Vec<String>);

impl From<DefaultLHTOutput> for FetchRsp {
    fn from(value: DefaultLHTOutput) -> Self {
        Self(value.into_iter().map(|d| BASE64_STANDARD.encode(d)).collect())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalHashTable(pub Vec<(String, Vec<String>)>);

#[derive(Serialize, Debug)]
pub enum DHTErr {
    FormatError(ApiFormatErr),
    SendError,
    Isolated,
    ReceiveError,
    Timeout,
    MessageReceiveMismatch,
    NotFound,
    Miscellaneous,
}

#[derive(Serialize, Debug)]
pub enum ApiFormatErr {
    HexFormatError,
    BoolFormatError,
    MissingParams(Vec<String>),
    AmbiguousParams,
}

impl Display for DHTErr {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FormatError(e) => write!(f, "Format Error: {}", e),
            Self::SendError => write!(f, "Error sending request."),
            Self::Isolated => write!(f, "Node is isolated."),
            Self::ReceiveError => write!(f, "Receive Error."),
            Self::Timeout => write!(f, "Timout of request."),
            Self::MessageReceiveMismatch => write!(f, "Response message received isn't expected type."),
            Self::NotFound => write!(f, "Unable to locate resource in the network."),
            Self::Miscellaneous => write!(f, "Unknown error occurred."),
        }
    }
}

impl IntoResponse for DHTErr {
    fn into_response(self) -> Response {
        let status = match self {
            // todo overthink status codes
            Self::FormatError(_) => http::StatusCode::BAD_REQUEST,
            Self::SendError => http::StatusCode::INTERNAL_SERVER_ERROR,
            Self::Isolated => http::StatusCode::SERVICE_UNAVAILABLE,
            Self::ReceiveError => http::StatusCode::BAD_GATEWAY,
            Self::Timeout => http::StatusCode::GATEWAY_TIMEOUT,
            Self::MessageReceiveMismatch => http::StatusCode::BAD_GATEWAY,
            Self::NotFound => http::StatusCode::NOT_FOUND,
            Self::Miscellaneous => http::StatusCode::SERVICE_UNAVAILABLE,
        };

        (status, self.to_string()).into_response()
    }
}

impl Display for ApiFormatErr {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HexFormatError => write!(f, "Supplied hex string is malformed."),
            Self::BoolFormatError => write!(f, "Expected boolean is not parsable."),
            Self::MissingParams(missing) => {
                if missing.len() == 1 {
                    write!(f, "Missing request parameter: {}", missing[0])
                } else {
                    write!(f, "Missing request parameters: {:?}", missing)
                }
            }
            Self::AmbiguousParams => write!(f, "Conflicting parameters provided. Parameters were passed that are mutually exclusive.")
        }
    }
}

impl From<ApiFormatErr> for DHTErr {
    fn from(value: ApiFormatErr) -> Self {
        Self::FormatError(value)
    }
}

impl From<FetchErr> for DHTErr {
    fn from(value: FetchErr) -> Self {
        match value {
            FetchErr::NotFoundErr => Self::NotFound,
        }
    }
}
