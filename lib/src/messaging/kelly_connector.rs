use std::sync::Arc;
use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;
use crate::domain::{NodeId, RoutingTable};
use crate::domain::api::NodeId as NodeIdApi;
use crate::messaging::source_route::SourceRoute;

pub trait KellyConnector {

    fn forward_request(&self, node_id: NodeId, source_route: SourceRoute);

    fn forward_response(&self, routing_table: crate::domain::api::RoutingTable);
}


#[derive(Serialize, Deserialize, Debug)]
pub struct SourcePath(Vec<crate::domain::api::NodeId>);

impl From<SourceRoute> for SourcePath {
    fn from(value: SourceRoute) -> Self {
         Self(value.ids().iter().map(|x| x.clone().into()).collect())
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct KellyRequest {
    node_id: String,
    source_path: Vec<String>
}

pub struct KellyResponse {
    routing_table: Vec<crate::domain::api::Bucket>
}


pub struct KellyConnectorImpl {
    client: reqwest::Client,
    address: String,
    runtime: Arc<Runtime>
}

impl KellyConnectorImpl {
    pub fn new(address: String, runtime: Arc<Runtime>) -> Self {
        Self {
            client: reqwest::Client::new(),
            address,
            runtime
        }
    }
}


impl KellyConnector for KellyConnectorImpl {

    fn forward_request(&self, node_id: NodeId, source_route: SourceRoute) {

        let id :NodeIdApi = node_id.into();
        log::warn!("Forwarding request to Kelly...");
        let body = KellyRequest {
            node_id: id.node_id,
            source_path: source_route.ids().iter().map(|x| <NodeId as Into<NodeIdApi>>::into(x.clone()).node_id).collect()
        };
        log::warn!("KeLLy address: {:?}", format!("{}/request", self.address.clone()));
        let cloned_client = self.client.clone();
        let cloned_address = self.address.clone();
        self.runtime.spawn(async move {
            cloned_client
            .post(format!("{}/request", cloned_address))
            .json(&body)
            .send().await.unwrap(); // TODO handle error
        });
    }

    fn forward_response(&self, routing_table: crate::domain::api::RoutingTable) {
        let cloned_client = self.client.clone();
        let cloned_address = self.address.clone();

        self.runtime.spawn(
            async move {
                cloned_client
                    .post(format!("{}/response", cloned_address))
                    .json(&routing_table)
                    .send().await.unwrap(); // TODO handle error
            }
        );
    }
}