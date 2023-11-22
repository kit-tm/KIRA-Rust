use std::fmt::Debug;
use serde::de::DeserializeOwned;
use serde::Serialize;
use crate::domain::NodeId;

pub mod data;

#[derive(Debug)]
pub struct StoreReqData<D: Serialize + Debug> {
    pub handle: NodeId,
    pub data: D,
    //store_duration: Duration,
    //replicate: bool
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