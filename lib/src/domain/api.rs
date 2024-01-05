use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use std::time::Duration;
use axum::http;
use hex::FromHexError;
use serde::{Deserialize, Serialize};
use crate::domain::dht::TimedValue;
use crate::messaging::dht::{DefaultLHTInput, FetchErr, StoreErr};
use crate::use_cases::{FetchInjectData, StoreInjectData};
use crate::use_cases::distributed_hash_table::{DefaultExpiringHashTable, HashTableSingle};
use axum::response::{IntoResponse, Response};


#[derive(Clone, Serialize, Deserialize, Debug, Hash, PartialEq, Eq)]
pub struct NodeId {
    #[serde(rename(serialize = "node-id", deserialize = "node-id"))]
    pub node_id: String,
}

impl From<crate::domain::NodeId> for NodeId {
    fn from(value: crate::domain::NodeId) -> Self {
        NodeId { node_id: format!("{}", value) }
    }
}

impl TryFrom<NodeId> for crate::domain::NodeId {
    type Error = FromHexError;

    fn try_from(value: NodeId) -> Result<Self, Self::Error> {
        Self::from_str(value.node_id.as_str())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StoreArgs {
    pub handle: String,

    #[serde(default = "restore_default")]
    pub restore: bool,
    pub data: DefaultLHTInput,
}

fn restore_default() -> bool {
    false
}


impl From<StoreInjectData<DefaultLHTInput>> for StoreArgs {
    fn from(value: StoreInjectData<DefaultLHTInput>) -> Self {
        let handle = NodeId::from(value.handle).node_id;
        Self {
            handle,
            restore: value.restore,
            data: value.data,
        }
    }
}

impl TryFrom<StoreArgs> for StoreInjectData<DefaultLHTInput>
{
    type Error = FromHexError;

    fn try_from(value: StoreArgs) -> Result<Self, Self::Error> {
        let handle = NodeId::try_into(NodeId { node_id: value.handle })?;
        Ok(Self {
            handle,
            data: value.data,
            restore: value.restore,
        })
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FetchArgs {
    pub handle: String,
}

impl From<FetchInjectData> for FetchArgs {
    fn from(value: FetchInjectData) -> Self {
        let handle = NodeId::from(value.handle).node_id;
        Self { handle }
    }
}

impl TryFrom<FetchArgs> for FetchInjectData {
    type Error = FromHexError;

    fn try_from(value: FetchArgs) -> Result<Self, Self::Error> {
        let handle = NodeId::try_into(NodeId { node_id: value.handle })?;
        Ok(Self { handle })
    }
}

pub enum StoreOK {
    Created,
    Updated,
}

impl Display for StoreOK {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Created => write!(f, "Created hashtable entry."),
            Self::Updated => write!(f, "Updated existing hashtable entry."),
        }
    }
}


impl From<crate::messaging::dht::StoreOK> for StoreOK {
    fn from(value: crate::messaging::dht::StoreOK) -> Self {
        match value {
            crate::messaging::dht::StoreOK::Created => Self::Created,
            crate::messaging::dht::StoreOK::Updated => Self::Updated
        }
    }
}

impl From<StoreErr> for ApiErr {
    fn from(value: StoreErr) -> Self {
        Self::Miscellaneous
    }
}

impl IntoResponse for StoreOK {
    fn into_response(self) -> Response {
        let status = match self {
            StoreOK::Created => http::StatusCode::CREATED,
            StoreOK::Updated => http::StatusCode::OK,
        };

        (status, self.to_string()).into_response()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalHashTable {
    #[serde(flatten)]
    ht: HashMap<String, Vec<TimedValue<String>>>,
}

impl From<HashTableSingle> for TimedValue<String> {
    fn from(value: HashTableSingle) -> Self {
        let time = value.time;
        let value = hex::encode(value.value);

        Self { value, time }
    }
}

impl From<DefaultExpiringHashTable> for LocalHashTable {
    fn from(value: DefaultExpiringHashTable) -> Self {
        Self {
            ht: value.map
                .into_iter()
                .map(|(k, v)| (NodeId::from(k).node_id, v.into_iter().map(Into::into).collect()))
                .collect()
        }
    }
}

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Serialize, Debug)]
pub enum ApiErr {
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
}

impl Display for ApiErr {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FormatError(e) => write!(f, "Format Error: {}", e),
            Self::SendError => write!(f, "Error sending request."),
            Self::Isolated => write!(f, "Node is isolated."),
            Self::ReceiveError => write!(f, "Receive Error."),
            Self::Timeout => write!(f, "Timout of request."),
            Self::MessageReceiveMismatch => write!(f, "Response message received isn't expected type."),
            Self::NotFound => write!(f, "Unable to locate resource in the network."),
            Self::Miscellaneous => write!(f, "Unknown error occurred.")
        }
    }
}

impl IntoResponse for ApiErr {
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
            ApiFormatErr::HexFormatError => write!(f, "Supplied hex string is malformed."),
            ApiFormatErr::BoolFormatError => write!(f, "Expected boolean is not parsable."),
            ApiFormatErr::MissingParams(missing) => {
                if missing.len() == 1 {
                    write!(f, "Missing request parameter: {}", missing[0])
                } else {
                    write!(f, "Missing request parameters: {:?}", missing)
                }
            }
        }
    }
}

impl From<ApiFormatErr> for ApiErr {
    fn from(value: ApiFormatErr) -> Self {
        Self::FormatError(value)
    }
}

impl From<FetchErr> for ApiErr {
    fn from(value: FetchErr) -> Self {
        match value {
            FetchErr::NotFoundErr => Self::NotFound,
            FetchErr::TimeOutErr => Self::Timeout
        }
    }
}