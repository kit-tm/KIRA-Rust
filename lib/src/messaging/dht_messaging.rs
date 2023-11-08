use std::time::Duration;
use serde::{Deserialize, Serialize};
use crate::domain::NodeId;

// todo remove in domain
pub enum DHTData {
    Single(u8),
    Slice([u8]),
    List(u8)
}
pub struct StoreReqData<D: Serialize> {
    handle: NodeId,
    data: D,
    //store_duration: Duration,
    //replicate: bool
}

pub enum StoreOK {
    Created,
    Updated
}

pub enum StoreErr {
    TimeOutErr,
    DataTypeErr
}

pub type StoreResult = Result<StoreOK, StoreErr>;

pub struct StoreRspData {
    status: StoreResult
}


pub struct FetchReqData {
    handle: NodeId
}

pub enum FetchErr {
    NotFoundErr,
    TimeOut
}
type FetchResult<D> = Result<D, FetchErr>;
pub struct FetchRspData<D: Deserialize> {
    data: FetchResult<D>
}