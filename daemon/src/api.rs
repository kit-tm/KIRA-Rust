use std::net::SocketAddr;
use std::ops::Add;
use axum::extract::State;
use axum::{Json, Router};
use axum::routing::{get, post};
use tokio::sync::mpsc;
use tokio::sync::mpsc::Sender;
use tokio::time::Instant;
use r2kad_lib::domain;
use r2kad_lib::domain::NodeId;
use r2kad_lib::use_cases::UseCaseEvent;


pub(crate) async fn start_http_server(api_config: ApiConfig) {
    log::info!("Starting api server on {}...", api_config.address);

    let api_state = ApiState {
        node_id: api_config.node_id,
        sender: api_config.sender
    };

    let app: Router = Router::new()
        .route("/node-id", get(get_node_id))

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

