use std::fmt::Debug;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use hex::{FromHex, FromHexError};
use serde::{Deserialize, Serialize};
use crate::messaging::dht::{DefaultLHTInput, DefaultLHTOutput, FetchErr, FetchReqData, FetchRspData, StoreResult, StoreRspData};
use crate::use_cases::{FetchInjectData, StoreInjectData};

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
pub struct StoreArgs {
    pub handle: String,

    #[serde(default = "restore_default")]
    pub restore: bool,
    pub data: String,
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
            data: hex::encode(value.data),
        }
    }
}

impl TryFrom<StoreArgs> for StoreInjectData<DefaultLHTInput>
{
    type Error = FromHexError;

    fn try_from(value: StoreArgs) -> Result<Self, Self::Error> {
        let handle = NodeId::try_into(NodeId { node_id: value.handle })?;
        let data = hex::decode(value.data)?;
        let data = data.into_boxed_slice().into();
        Ok(Self {
            handle,
            data,
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

#[derive(Debug, Clone, Serialize)]
pub struct StoreRsp {
    #[serde(flatten)]
    status: StoreResult
}

impl From<StoreRspData> for StoreRsp {
    fn from(value: StoreRspData) -> Self {
        Self { status: value.status}
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FetchRsp {
    #[serde(flatten)]
    data: Result<Vec<String>, FetchErr>
}

impl From<FetchRspData<DefaultLHTOutput>> for FetchRsp{
    fn from(value: FetchRspData<DefaultLHTOutput>) -> Self {
        let data = value.data
            .map(|data| data.into_iter().map(hex::encode).collect());
        Self { data }
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