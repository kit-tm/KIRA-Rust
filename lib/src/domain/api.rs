use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct NodeId {
    #[serde(rename(serialize = "node-id", deserialize = "node-id"))]
    pub node_id: String
}

#[derive(Serialize)]
pub struct Contact {
    node_id: NodeId,
    node_degree: Option<usize>,
    physical_neighbor_id_sum: Option<NodeId>,
    path_vector: Vec<NodeId>
}

#[derive(Serialize)]
pub struct Bucket {
    pub(crate) prefix: String,
    pub(crate) contacts: Vec<Contact>,
}

#[derive(Serialize)]
pub struct RoutingTable {
    pub node_id: NodeId,
    pub buckets: Vec<Bucket>
}

impl From<crate::domain::NodeId> for NodeId {
    fn from(value: crate::domain::NodeId) -> Self {
        NodeId { node_id: format!("{}", value) }
    }
}
impl From<crate::domain::Contact> for Contact {
    fn from(value: crate::domain::Contact) -> Self {
        Self {
            node_id: value.id().clone().into(),
            node_degree: value.number_of_pn().clone(),
            physical_neighbor_id_sum: value.neighbor_sum().clone().map(|x| x.into()),
            path_vector: value.path().into_iter().map(|x| x.clone().into()).collect(),
        }
    }
}

impl From<crate::domain::Bucket> for Bucket {
    fn from(value: crate::domain::Bucket) -> Self {
        Bucket {
            prefix: "".to_string(),
            contacts: value.into_iter().map(|x| x.into()).collect(),
        }
    }
}

impl From<Vec<crate::domain::Bucket>> for Bucket {
    fn from(value: Vec<crate::domain::Bucket>) -> Self {
        Bucket {
            prefix: "".to_string(),
            contacts: value.into_iter().map(|x| x.into_iter().map(|y| y.into())).flatten().collect(),
        }
    }
}
