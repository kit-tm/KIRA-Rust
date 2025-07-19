use crate::api::domain::NodeId;
use axum::http;
use axum::response::{IntoResponse, Response};
use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use hex::FromHexError;
#[cfg(feature = "swagger_doc")]
use itertools::Itertools;
use kira_r2kad::domain::SIZE;
use kira_r2kad::messaging::dht::{DefaultLHTInput, DefaultLHTOutput, FetchErr, StoreErr};
use kira_r2kad::use_cases::{FetchInjectData, StoreInjectData};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fmt::{Display, Formatter};
use std::time::Duration;
#[cfg(feature = "swagger_doc")]
use utoipa::openapi::{RefOr, ResponseBuilder, ResponsesBuilder};
#[cfg(feature = "swagger_doc")]
use utoipa::{ToResponse, ToSchema};

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

pub enum Handle {
    Handle(NodeId),
    Key(String),
}

impl TryFrom<Handle> for kira_r2kad::domain::NodeId {
    type Error = FromHexError;

    fn try_from(value: Handle) -> Result<Self, Self::Error> {
        match value {
            Handle::Handle(handle) => handle.try_into(),
            Handle::Key(key) => {
                // calculating SHA256 hash
                let hash = Sha256::digest(key.as_bytes());
                let handle: [u8; SIZE] = hash
                    .into_iter()
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

impl TryFrom<StoreArgs> for StoreInjectData<DefaultLHTInput> {
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

#[derive(Debug)]
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

impl From<kira_r2kad::messaging::dht::StoreOK> for StoreOK {
    fn from(value: kira_r2kad::messaging::dht::StoreOK) -> Self {
        match value {
            kira_r2kad::messaging::dht::StoreOK::Created => Self::Created,
            kira_r2kad::messaging::dht::StoreOK::Updated => Self::Updated,
            kira_r2kad::messaging::dht::StoreOK::Inserted => Self::Inserted,
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
            StoreOK::Created | StoreOK::Inserted => http::StatusCode::CREATED,
            StoreOK::Updated => http::StatusCode::OK,
        };

        (status, self.to_string()).into_response()
    }
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "swagger_doc", derive(ToSchema, ToResponse))]
pub struct FetchRsp(Vec<String>);

impl From<DefaultLHTOutput> for FetchRsp {
    fn from(value: DefaultLHTOutput) -> Self {
        Self(
            value
                .into_iter()
                .map(|d| BASE64_STANDARD.encode(d))
                .collect(),
        )
    }
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "swagger_doc", derive(ToSchema))]
pub struct LocalHashTable(pub HashMap<String, Vec<String>>);

#[derive(Serialize, Debug, Clone)]
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

#[derive(Serialize, Debug, Clone)]
pub enum ApiFormatErr {
    HexFormatError,
    BoolFormatError,
    MissingParams(Vec<String>),
    AmbiguousParams,
}

impl Display for DHTErr {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FormatError(e) => write!(f, "Format Error: {e}"),
            Self::SendError => write!(f, "Error sending request."),
            Self::Isolated => write!(f, "Node is isolated."),
            Self::ReceiveError => write!(f, "Receive Error."),
            Self::Timeout => write!(f, "Timout of request."),
            Self::MessageReceiveMismatch => {
                write!(f, "Response message received isn't expected type.")
            }
            Self::NotFound => write!(f, "Unable to locate resource in the network."),
            Self::Miscellaneous => write!(f, "Unknown error occurred."),
        }
    }
}

impl IntoResponse for DHTErr {
    fn into_response(self) -> Response {
        let status = match self {
            // TODO: overthink status codes
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
                    write!(f, "Missing request parameters: {missing:?}")
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

#[cfg(feature = "swagger_doc")]
// creates responses based on a selected example list
fn responses<R: IntoResponse + Display>(
    examples: Vec<R>,
) -> BTreeMap<String, RefOr<utoipa::openapi::Response>> {
    let binding = examples
        .into_iter()
        .map(|e| (e.to_string(), e.into_response().status()))
        // group status codes so we can display all alternatives that throw same status code
        .chunk_by(|(_, status)| *status);
    let iter = binding.into_iter().map(|(status_code, e)| {
        (
            status_code.to_string(),
            ResponseBuilder::new().description(
                e
                    // strip common part of FormatError
                    .map(|(description, _)| {
                        description.find(':').map_or(description.clone(), |pos| {
                            description[pos + 1..].trim_start().to_string()
                        })
                    })
                    .join(" | "),
            ),
        )
    });

    ResponsesBuilder::new()
        .responses_from_iter(iter)
        .build()
        .into()
}

#[cfg(feature = "swagger_doc")]
impl utoipa::IntoResponses for DHTErr {
    fn responses() -> BTreeMap<String, RefOr<utoipa::openapi::Response>> {
        let examples = vec![
            DHTErr::FormatError(ApiFormatErr::AmbiguousParams),
            DHTErr::FormatError(ApiFormatErr::HexFormatError),
            DHTErr::FormatError(ApiFormatErr::MissingParams(vec![
                "missing_parameter".to_string()
            ])),
            DHTErr::FormatError(ApiFormatErr::MissingParams(vec![
                "missing_parameter1".to_string(),
                "missing_parameter2".to_string(),
            ])),
            DHTErr::FormatError(ApiFormatErr::BoolFormatError),
            DHTErr::SendError,
            DHTErr::Isolated,
            DHTErr::ReceiveError,
            DHTErr::Timeout,
            DHTErr::MessageReceiveMismatch,
            DHTErr::NotFound,
            DHTErr::Miscellaneous,
        ];

        responses(examples)
    }
}

#[cfg(feature = "swagger_doc")]
impl utoipa::IntoResponses for StoreOK {
    fn responses() -> BTreeMap<String, RefOr<utoipa::openapi::Response>> {
        let examples = vec![StoreOK::Inserted, StoreOK::Created, StoreOK::Updated];

        responses(examples)
    }
}

#[cfg(feature = "swagger_doc")]
impl utoipa::IntoResponses for FetchRsp {
    fn responses() -> BTreeMap<String, RefOr<utoipa::openapi::Response>> {
        ResponsesBuilder::new()
            .response("200", Self::response().1)
            .build()
            .into()
    }
}
