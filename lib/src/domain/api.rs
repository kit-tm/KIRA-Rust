use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct NodeId {
    #[serde(rename(serialize = "node-id", deserialize = "node-id"))]
    pub node_id: String
}

impl From<crate::domain::NodeId> for NodeId {
    fn from(value: crate::domain::NodeId) -> Self {
        NodeId { node_id: format!("{}", value) }
    }
}
