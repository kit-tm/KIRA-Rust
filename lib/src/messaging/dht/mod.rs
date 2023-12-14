use std::fmt::Debug;
use std::sync::Arc;
use serde::Deserialize;
use crate::domain::NodeId;

pub type DefaultLHTInput = Arc<[u8]>;
pub type DefaultLHTOutput = Vec<Arc<[u8]>>;

#[derive(Debug, PartialEq, Eq, Clone, Deserialize)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct StoreReqData<D: Debug> {
    pub handle: NodeId,
    pub data: D,
    //store_duration: Duration,
}

#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum StoreOK {
    Created,
    Updated,
}

#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum StoreErr {
    TimeOutErr,
    DataTypeErr,
}

pub type StoreResult = Result<StoreOK, StoreErr>;

#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct StoreRspData {
    pub status: StoreResult,
}


#[derive(Debug, PartialEq, Eq, Clone, Deserialize)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct FetchReqData {
    pub handle: NodeId,
}


#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum FetchErr {
    NotFoundErr,
    TimeOutErr,
}

#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct FetchRspData<D: Debug> {
    pub data: Result<D, FetchErr>,
}