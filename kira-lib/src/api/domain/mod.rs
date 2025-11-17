use std::num::ParseIntError;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
#[cfg(feature = "swagger_doc")]
use utoipa::ToSchema;

pub mod dht;

#[derive(Clone, Serialize, Deserialize, Debug, Hash, PartialEq, Eq)]
#[cfg_attr(feature = "swagger_doc", derive(ToSchema))]
pub struct NodeId {
    #[serde(rename(serialize = "node-id", deserialize = "node-id"))]
    pub node_id: String,
}

impl From<kira_r2kad::domain::NodeId> for NodeId {
    fn from(value: kira_r2kad::domain::NodeId) -> Self {
        NodeId {
            node_id: format!("{value}"),
        }
    }
}

impl TryFrom<NodeId> for kira_r2kad::domain::NodeId {
    type Error = ParseIntError;

    fn try_from(value: NodeId) -> Result<Self, Self::Error> {
        Self::from_str(value.node_id.as_str())
    }
}
