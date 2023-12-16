use std::time::Duration;
use serde::{Deserialize, Serialize};

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

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Serialize)]
pub enum ApiErr {
    SendError,
    Isolated,
    ReceiveError,
    Timeout,
    MessageReceiveMismatch
}