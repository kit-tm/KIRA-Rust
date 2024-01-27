use serde::{Deserialize, Serialize};
use crate::messaging;


#[derive(Debug, PartialEq, Eq, Clone, Hash, Serialize, Deserialize)]
pub struct Nonce(pub u128);

impl Nonce {
    pub fn random() -> Self {
        Self(rand::random())
    }
}

impl From<messaging::Nonce> for Nonce {
    fn from(value: messaging::Nonce) -> Self {
        Self(value.0)
    }
}

impl From<Nonce> for messaging::Nonce {
    fn from(value: Nonce) -> Self {
        Self(value.0)
    }
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct NodeIdApi {
    #[serde(rename(serialize = "node-id", deserialize = "node-id"))]
    pub node_id: String
}

#[derive(Serialize, Deserialize, Debug)]
pub struct KellyRequest {
    pub(crate) node_id: String,
    pub(crate) nonce: Nonce,
    pub(crate) source_path: Vec<String>
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct KellyResponse {
    pub nonce: Nonce,
    pub node: NodeApi,
    pub discovery_range: DiscoveryRange,
    pub routing_table: RoutingTable
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct OutgoingKellyRequest {
    pub(crate) node_id: String,
    pub(crate) nonce: Nonce
}


#[derive(Debug, Serialize, Deserialize)]
pub struct OutgoingKellyResponse {
    pub response: KellyResponse,
    pub route: Vec<String>
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RoutingTableResponse {
    pub node: NodeApi,
    pub discovery_range: DiscoveryRange,
    pub routing_table: RoutingTable
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RoutingTable {
    pub layers: Vec<RoutingTableLayer>,
    pub acceleration_factor: usize,
    pub bucket_size: usize
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RoutingTableLayer {
    pub buckets: Vec<Bucket>
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct DiscoveryRange {
    pub start: NodeIdApi,
    pub end: NodeIdApi
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct NodeApi {
    pub id: NodeIdApi,
    pub neighbor_id_sum: NodeIdApi,
    pub degree: usize
}

#[derive(Debug, Clone, Deserialize,Serialize, PartialEq, Eq)]
pub struct Contact {
    node_id: NodeIdApi,
    node_degree: Option<usize>,
    physical_neighbor_id_sum: Option<NodeIdApi>,
    path_vector: Vec<NodeIdApi>
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct Bucket {
    pub(crate) prefix: String,
    pub(crate) contacts: Vec<Contact>,
}

impl From<crate::domain::NodeId> for NodeIdApi {
    fn from(value: crate::domain::NodeId) -> Self {
        NodeIdApi { node_id: format!("{}", value) }
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

impl<const BUCKET_SIZE: usize> From<crate::domain::Bucket<BUCKET_SIZE>> for Bucket {
    fn from(value: crate::domain::Bucket<BUCKET_SIZE>) -> Self {
        Bucket {
            prefix: "".to_string(),
            contacts: value.into_iter().map(|x| x.into()).collect(),
        }
    }
}

impl<const BUCKET_SIZE: usize> From<Vec<crate::domain::Bucket<BUCKET_SIZE>>> for Bucket {
    fn from(value: Vec<crate::domain::Bucket<BUCKET_SIZE>>) -> Self {
        Bucket {
            prefix: "".to_string(),
            contacts: value.into_iter().map(|x| x.into_iter().map(|y| y.into())).flatten().collect(),
        }
    }
}
