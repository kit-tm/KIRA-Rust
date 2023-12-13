use std::sync::Arc;
use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;
use crate::domain::{NodeId, RoutingTable};
use crate::domain::api::{KellyRequest, KellyResponse, NodeIdApi as NodeIdApi};
use crate::messaging::Nonce;
use crate::messaging::source_route::SourceRoute;

pub trait KellyConnector {

    fn forward_request(&self, node_id: NodeId, source_route: SourceRoute, nonce: crate::domain::api::Nonce);

    fn forward_response(&self, response: KellyResponse);
}


#[derive(Serialize, Deserialize, Debug)]
pub struct SourcePath(Vec<crate::domain::api::NodeIdApi>);

impl From<SourceRoute> for SourcePath {
    fn from(value: SourceRoute) -> Self {
         Self(value.ids().iter().map(|x| x.clone().into()).collect())
    }
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

    fn forward_request(&self, node_id: NodeId, source_route: SourceRoute, nonce: crate::domain::api::Nonce) {

        let id :NodeIdApi = node_id.into();
        log::warn!("Forwarding request to Kelly...");
        let body = KellyRequest {
            node_id: id.node_id,
            nonce,
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

    fn forward_response(&self, response: KellyResponse) {
        let cloned_client = self.client.clone();
        let cloned_address = self.address.clone();

        self.runtime.spawn(
            async move {
                cloned_client
                    .post(format!("{}/response", cloned_address))
                    .json(&response)
                    .send().await.unwrap(); // TODO handle error
            }
        );
    }
}