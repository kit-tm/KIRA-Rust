use std::net::SocketAddr;
use std::ops::Add;
use axum::extract::State;
use axum::{Json, Router};
use axum::routing::get;
use tokio::sync::mpsc;
use tokio::sync::mpsc::Sender;
use tokio::time::Instant;
use r2kad_lib::domain::api::{RoutingTable};
use r2kad_lib::domain::NodeId;
use r2kad_lib::use_cases::{ApiEvent, UseCaseEvent};

pub(crate) async fn start_http_server(api_config: ApiConfig) {
    log::info!("Starting api server on {}...", api_config.address);

    let api_state = ApiState {
        node_id: api_config.node_id,
        sender: api_config.sender
    };

    let app: Router = Router::new()
        .route("/node-id", get(get_node_id))
        .route("/r2kademlia/stacks/default/node-id", get(get_node_id_for_test_env))
        .route("/routing-table", get(get_routing_table))
        .with_state(api_state);

    axum::Server::bind(&api_config.address)
        .serve(app.into_make_service())
        .await
        .unwrap()
}

#[derive(Clone)]
struct ApiState {
    node_id: NodeId,
    sender: Sender<(UseCaseEvent, Option<Instant>)>
}

pub struct ApiConfig {
    address: SocketAddr,
    node_id: NodeId,
    sender: Sender<(UseCaseEvent, Option<Instant>)>
}

impl ApiConfig {

    pub(crate) fn new(address: SocketAddr, node_id: NodeId, sender: Sender<(UseCaseEvent, Option<Instant>)>) -> ApiConfig {
        ApiConfig {
            address,
            node_id,
            sender
        }
    }
}

async fn get_node_id(State(state): State<ApiState>) -> Json<r2kad_lib::domain::api::NodeId> {
    Json(state.node_id.into())
}

async fn get_node_id_for_test_env(State(state): State<ApiState>) -> String {
    let test: String = state.node_id.bytes().iter().map(|x| format!("\\x{:02x}", x)).collect::<Vec<String>>().concat();
    let mut result = "{ \"node-id\": \"".to_string();
    result = result.add(&test);
    result = result.add("\" }");

    result
}

async fn get_routing_table(State(state): State<ApiState>) -> Json<RoutingTable> {
    let (tx, mut rx) = mpsc::unbounded_channel::<RoutingTable>();

    let res = state.sender.send((UseCaseEvent::API(ApiEvent::RoutingTable(tx)), None)).await;

    let result = rx.recv().await.unwrap();
    Json(result)

}

