use std::collections::HashMap;
use std::fmt::Debug;
use std::str::FromStr;
use std::time::Duration;
use hex::FromHexError;
use serde::{Deserialize, Serialize};
use crate::domain::dht::TimedValue;
use crate::messaging::dht::{DefaultLHTInput, DefaultLHTOutput, FetchErr, FetchRspData, StoreResult, StoreRspData};
use crate::use_cases::{FetchInjectData, StoreInjectData};
use crate::use_cases::distributed_hash_table::{DefaultExpiringHashTable, HashTableData, HashTableSingle};

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

#[derive(Debug, Clone, Serialize)]
pub struct LocalHashTable {
    #[serde(flatten)]
    ht: HashMap<String, Vec<TimedValue<String>>>
}

impl From<HashTableSingle> for TimedValue<String> {
    fn from(value: HashTableSingle) -> Self {
        let time = value.time;
        let value = hex::encode(value.value);

        Self { value, time}
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

#[derive(Serialize)]
pub enum ApiErr {
    FormatError(ApiFormatErr),
    SendError,
    Isolated,
    ReceiveError,
    Timeout,
    MessageReceiveMismatch,
}

#[derive(Serialize)]
pub enum ApiFormatErr {
    HexFormatError,
    BoolFormatError,
    MissingParam(String),
    MissingParams(Vec<String>)
}

impl From<ApiFormatErr> for ApiErr {
    fn from(value: ApiFormatErr) -> Self {
        Self::FormatError(value)
    }
}