use hex::FromHexError;
use serde_derive::{Deserialize, Serialize};
use std::str::FromStr;

pub mod dht;


#[derive(Clone, Serialize, Deserialize, Debug, Hash, PartialEq, Eq)]
pub struct NodeId {
    #[serde(rename(serialize = "node-id", deserialize = "node-id"))]
    pub node_id: String,
}

impl From<r2kad_lib::domain::NodeId> for NodeId {
    fn from(value: r2kad_lib::domain::NodeId) -> Self {
        NodeId { node_id: format!("{}", value) }
    }
}

impl TryFrom<NodeId> for r2kad_lib::domain::NodeId {
    type Error = FromHexError;

    fn try_from(value: NodeId) -> Result<Self, Self::Error> {
        Self::from_str(value.node_id.as_str())
    }
}
