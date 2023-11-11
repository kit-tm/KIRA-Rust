use std::fmt::{Debug, Formatter};
use serde::{Serialize};
use serde::de::DeserializeOwned;
use crate::domain::NodeId;
use crate::messaging::{ProtocolMessage, ReqRspMessage};

// todo implement Error on Error states
// todo derive sensible traits
pub struct StoreReqData<D: Serialize> {
    pub handle: NodeId,
    pub data: D,
    //store_duration: Duration,
    //replicate: bool
}

impl<D> From<ReqRspMessage<StoreReqData<D>>> for ProtocolMessage::StoreReq {
    fn from(data: ReqRspMessage<StoreReqData<D>>) -> Self {
        Self(data)
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

impl From<ReqRspMessage<StoreRspData>> for ProtocolMessage::StoreRsp {
    fn from(data: ReqRspMessage<StoreRspData>) -> Self {
        Self(data)
    }
}


pub struct FetchReqData {
    handle: NodeId,
}

#[derive(Debug)]
pub enum FetchErr {
    NotFoundErr,
    TimeOut,
}

pub struct FetchRspData<D: DeserializeOwned> {
    pub data: Result<D, FetchErr>,
}

impl<D: DeserializeOwned> Debug for FetchRspData<D> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("FetchRspData");

        let mut s = match &self.data { // todo maybe improve this
            Ok(_) => s.field("Data", "[rawdata]"),
            Err(e) => s.field("FetchErr", e)
        };

        s.finish()
    }
}