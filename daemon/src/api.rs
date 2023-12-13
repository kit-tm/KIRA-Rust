use std::net::SocketAddr;
use std::ops::Add;
use axum::extract::State;
use axum::{Json, Router};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::sync::mpsc::Sender;
use tokio::time::Instant;
use r2kad_lib::context::UseCaseContext;
use r2kad_lib::domain;
use r2kad_lib::domain::api::{KellyRequest, NodeIdApi, OutgoingKellyRequest, OutgoingKellyResponse, RoutingTable, RoutingTableResponse};
use r2kad_lib::domain::NodeId;
use r2kad_lib::use_cases::{ApiEvent, UseCaseEvent};
use r2kad_lib::use_cases::ApiEvent::SendKellyReq;

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
        .route("/kelly/request", post(send_kelly_request))
        .route("/kelly/response", post(send_kelly_response))
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

#[derive(Serialize, Deserialize, Debug)]
struct KellyResponse {
    route: Vec<domain::api::NodeIdApi>,
    routing_table: RoutingTable
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

async fn get_node_id(State(state): State<ApiState>) -> Json<r2kad_lib::domain::api::NodeIdApi> {
    Json(state.node_id.into())
}

async fn get_node_id_for_test_env(State(state): State<ApiState>) -> String {
    let test: String = state.node_id.bytes().iter().map(|x| format!("\\x{:02x}", x)).collect::<Vec<String>>().concat();
    let mut result = "{ \"node-id\": \"".to_string();
    result = result.add(&test);
    result = result.add("\" }");

    result
}

async fn get_routing_table(State(state): State<ApiState>) -> Json<RoutingTableResponse> {
    log::warn!("Returning routing table via api");
    let (tx, mut rx) = mpsc::unbounded_channel::<RoutingTableResponse>();

    let res = state.sender.send((UseCaseEvent::API(ApiEvent::RoutingTable(tx)), None)).await;

    let result = rx.recv().await.unwrap();
    log::warn!("Got routing table, sending: {:?}", result);
    Json(result)

}

async fn send_kelly_request(State(state): State<ApiState>, Json(request): Json<OutgoingKellyRequest>) {

    log::warn!("Received api send kelly.py request for Node {:?}", request);

    let result = state.sender.send((UseCaseEvent::API(SendKellyReq(request)), None)).await;

    if let Err(e) = result {
        log::error!("Failed to send Kelly Request: {}", e)
    } else {
        log::info!("Sending successful");
    }
    
}

async fn send_kelly_response(State(state): State<ApiState>, Json(response): Json<OutgoingKellyResponse>) {

    log::warn!("Received api send kelly.py response message");

    let result = state.sender.send((
        UseCaseEvent::API(ApiEvent::SendKellyRsp(response.route.iter().map(|x| NodeIdApi {node_id: x.clone()}).collect(), response.response)),
        None)).await;

}

