use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;
use crate::domain::NodeId;
use crate::messaging::source_route::SourceRoute;

pub trait KellyConnector {

    fn forward_request(&self, node_id: NodeId, source_route: SourceRoute);

    fn forward_response(&self);
}


#[derive(Serialize, Deserialize, Debug)]
pub struct SourcePath(Vec<crate::domain::api::NodeId>);

impl From<SourceRoute> for SourcePath {
    fn from(value: SourceRoute) -> Self {
        Self(value.ids())
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct KellyRequest(crate::domain::api::NodeId, SourcePath);



pub struct KellyConnectorImpl {
    client: reqwest::Client,
    address: String,
    runtime: Runtime
}

impl KellyConnectorImpl {
    fn new(address: String, runtime: Runtime) -> Self {
        Self {
            client: reqwest::Client::new(),
            address,
            runtime
        }
    }
}


impl KellyConnector for KellyConnectorImpl {

    fn forward_request(&self, node_id: NodeId, source_route: SourceRoute) {
        let body = KellyRequest(node_id.into(), source_route.into());
        self.runtime.spawn(|| self.client
            .post(self.address.clone() + "/request")
            .body(body)
            .send());
    }

    fn forward_response(&self) {
        self.runtime.spawn(|| self.client
            .post(self.address.clone() + "/response")
            .body(todo!())
            .send());
    }
}