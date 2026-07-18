//! Data types for messages used to interact with the distributed hash table.

use crate::domain::Age;
use crate::domain::NodeId;
use crate::messaging::ProtocolMessage;
use crate::messaging::ReqRspMessage;
use std::fmt::Debug;
use std::sync::Arc;

pub type LHTInput = Arc<[u8]>;
pub type LHTOutput = Vec<Arc<[u8]>>;

/// Data struct representing a StoreReq protocol message.
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct StoreReqData<D> {
    /// The handle with which the data can be retrieved later.
    pub handle: NodeId,
    /// The data to save with this request.
    pub data: D,
    /// Last time the key-value pair was accessed.
    ///
    /// This is set if a key-value pair is _republished_.
    #[cfg_attr(feature = "serde", serde(default))]
    pub last_accessed_ms: Option<Age>,
}

impl From<ReqRspMessage<StoreReqData<LHTInput>>> for ProtocolMessage {
    fn from(message: ReqRspMessage<StoreReqData<LHTInput>>) -> Self {
        ProtocolMessage::StoreReq(message)
    }
}

/// Successful storage of hash table data.
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum StoreOk {
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
    /// An unexpected error occurred on storing a key-value pair in the DHT.
    UnexpectedError(String),
}

/// The result returned by the store response.
///
/// This result is wrapped in the [StoreRspData] struct.
pub type StoreResult = Result<StoreOk, StoreErr>;

/// Data struct representing a StoreRsp protocol message.
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct StoreRspData {
    /// Result of the StoreRsp
    pub status: StoreResult,
}

impl From<ReqRspMessage<StoreRspData>> for ProtocolMessage {
    fn from(message: ReqRspMessage<StoreRspData>) -> Self {
        ProtocolMessage::StoreRsp(message)
    }
}

/// Data struct representing a FetchReq protocol message.
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct FetchReqData {
    /// Handle of which the sender wants to know the stored data.
    pub handle: NodeId,
}

impl From<ReqRspMessage<FetchReqData>> for ProtocolMessage {
    fn from(message: ReqRspMessage<FetchReqData>) -> Self {
        ProtocolMessage::FetchReq(message)
    }
}

/// Errors that may occur on a FetchReq protocol message.
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

impl From<ReqRspMessage<FetchRspData<LHTOutput>>> for ProtocolMessage {
    fn from(message: ReqRspMessage<FetchRspData<LHTOutput>>) -> Self {
        ProtocolMessage::FetchRsp(message)
    }
}
