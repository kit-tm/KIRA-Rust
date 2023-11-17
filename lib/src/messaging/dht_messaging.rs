use std::fmt::Debug;
use serde::Serialize;
use serde::de::DeserializeOwned;
use crate::domain::NodeId;
use crate::messaging::{ProtocolMessage, ReqRspMessage};

// todo implement Error on Error states
#[derive(Debug)]
pub struct StoreReqData<D: Serialize + Debug> {
    pub handle: NodeId,
    pub data: D,
    //store_duration: Duration,
    //replicate: bool
}

impl<D: Serialize + Debug> From<ReqRspMessage<StoreReqData<D>>> for ProtocolMessage {
    fn from(data: ReqRspMessage<StoreReqData<D>>) -> Self {
        Self::StoreReq(data)
    }
}

pub enum StoreOK {
    Created,
    Updated,
}

pub enum StoreErr {
    TimeOutErr,
    DataTypeErr,
}

pub type StoreResult = Result<StoreOK, StoreErr>;

#[derive(Debug)]
pub struct StoreRspData {
    pub status: StoreResult,
}

impl From<ReqRspMessage<StoreRspData>> for ProtocolMessage {
    fn from(data: ReqRspMessage<StoreRspData>) -> Self {
        Self::StoreRsp(data)
    }
}


#[derive(Debug)]
pub struct FetchReqData {
    handle: NodeId,
}

#[derive(Debug)]
pub enum FetchErr {
    NotFoundErr,
    TimeOut,
}

#[derive(Debug)]
pub struct FetchRspData<D: DeserializeOwned + Debug> {
    pub data: Result<D, FetchErr>,
}
impl<D: DeserializeOwned + Debug> From<ReqRspMessage<FetchRspData<D>>> for ProtocolMessage {
    fn from(data: ReqRspMessage<FetchRspData<D>>) -> Self {
        Self::FetchRsp(data)
    }
}