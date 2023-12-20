use std::fmt::Debug;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use hex::{FromHex, FromHexError};
use serde::{Deserialize, Serialize};
use crate::messaging::dht::{DefaultLHTInput, FetchReqData};

#[derive(Clone, Serialize, Deserialize, Debug)]
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
pub struct StoreApiData {
    pub handle: String,

    #[serde(default = "restore_default")]
    pub restore: bool,
    pub data: String,
}

fn restore_default() -> bool {
    false
}


impl From<crate::use_cases::StoreInjectData<DefaultLHTInput>> for StoreApiData {
    fn from(value: crate::use_cases::StoreInjectData<DefaultLHTInput>) -> Self {
        let handle = NodeId::from(value.data.handle).node_id;
        Self {
            handle,
            restore: value.restore,
            data: hex::encode(value.data.data),
        }
    }
}

impl TryFrom<StoreApiData> for crate::use_cases::StoreInjectData<DefaultLHTInput>
{
    type Error = FromHexError;

    fn try_from(value: StoreApiData) -> Result<Self, Self::Error> {
        let handle = NodeId::try_into(NodeId { node_id: value.handle })?;
        let data = hex::decode(value.data)?;
        let data = data.into_boxed_slice().into();
        Ok(Self {
            data: crate::messaging::dht::StoreReqData {
                handle,
                data,
            },
            restore: value.restore,
        })
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FetchApiData {
    pub handle: String,
}

impl From<FetchReqData> for FetchApiData {
    fn from(value: FetchReqData) -> Self {
        let handle = NodeId::from(value.handle).node_id;
        Self { handle }
    }
}

impl TryFrom<FetchApiData> for FetchReqData {
    type Error = FromHexError;

    fn try_from(value: FetchApiData) -> Result<Self, Self::Error> {
        let handle = NodeId::try_into(NodeId { node_id: value.handle })?;
        Ok(Self { handle })
    }
}

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Serialize)]
pub enum ApiErr {
    HexFormatError,
    SendError,
    Isolated,
    ReceiveError,
    Timeout,
    MessageReceiveMismatch,
}