//! Data types for messages used to interact with the distributed hash table.

use std::fmt::Debug;
use std::sync::Arc;
use crate::domain::NodeId;

pub type DefaultLHTInput = Arc<[u8]>;
pub type DefaultLHTOutput = Vec<Arc<[u8]>>;


/// Data struct representing a StoreReq protocol message.
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct StoreReqData<D: Debug> {
    /// The handle with which the data can be retrieved later.
    pub handle: NodeId,
    /// The data to save with this request.
    pub data: D,
    //store_duration: Duration,
}

/// Successful storage of hash table data.
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum StoreOK {
    /// No previous data was stored under the specified handle.
    Created,
    /// Data was appended to existing data-entry.
    Inserted,
    /// The exact data already existed and only the timestamp was updated.
    ///
    /// This is usually expected to be returned on periodic restores.
    Updated,
}


/// A [StoreErr] should never be returned under the current implementation,
/// since all store requests should succeed.
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum StoreErr {
}


/// The result returned by the store response.
///
/// This result is wrapped in the [StoreRspData] struct.
pub type StoreResult = Result<StoreOK, StoreErr>;

/// Data struct representing a StoreRsp protocol message.
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct StoreRspData {
    /// Result of the StoreRsp
    pub status: StoreResult,
}


/// Data struct representing a FetchReq protocol message.
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct FetchReqData {
    /// Handle of which the sender wants to know the stored data.
    pub handle: NodeId,
}

/// Errors that may occur on a [FetchReq] protocol message.
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum FetchErr {
    /// No data is stored under the given handle.
    NotFoundErr,
}

/// Data struct representing a FetchRsp protocol message.
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct FetchRspData<D: Debug> {
    /// The resulting data if successfully found or a [FetchErr]
    /// further describing the error that occurred while trying to fetch
    /// the data.
    pub data: Result<D, FetchErr>,
}