use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::{Json, Router};
use axum::routing::{get, post};
use axum_macros::debug_handler;

use tokio::time::Instant;
use tokio::sync::mpsc;

use r2kad_lib::domain::NodeId;
use r2kad_lib::domain::api::ApiErr;
use r2kad_lib::use_cases::inject_messages::InjectionResult;
use r2kad_lib::use_cases::{InjectionMessageData, StoreInjectData, UseCaseEvent};
use r2kad_lib::messaging::dht::{DefaultLHTInput, DefaultLHTOutput, FetchReqData, FetchRspData, StoreReqData, StoreRspData};
use r2kad_lib::messaging::{Nonce, ProtocolMessage};


pub(crate) async fn start_http_server(api_config: ApiConfig) {
    log::info!("Starting api server on {}...", api_config.address);

    let api_state = ApiState {
        node_id: api_config.node_id,
        sender: api_config.sender,
    };

    let app: Router = Router::new()
        .route("/node-id", get(get_node_id))
        .route("/dht/store", post(store_dht_data))
        .route("/dht/dev/store_example", get(store_dht_data_example))
        .route("/dht/fetch", post(fetch_dht_data))
        .route("/dht/dev/fetch_example", get(fetch_dht_data_example))
        .with_state(api_state);

    axum::Server::bind(&api_config.address)
        .serve(app.into_make_service())
        .await
        .unwrap()
}

#[derive(Clone)]
struct ApiState {
    node_id: NodeId,
    sender: mpsc::Sender<(UseCaseEvent, Option<Instant>)>,
}

pub struct ApiConfig {
    address: SocketAddr,
    node_id: NodeId,
    sender: mpsc::Sender<(UseCaseEvent, Option<Instant>)>,
}

impl ApiConfig {
    pub(crate) fn new(address: SocketAddr, node_id: NodeId, sender: mpsc::Sender<(UseCaseEvent, Option<Instant>)>) -> ApiConfig {
        ApiConfig {
            address,
            node_id,
            sender,
        }
    }
}

async fn get_node_id(State(state): State<ApiState>) -> Json<r2kad_lib::domain::api::NodeId> {
    Json(state.node_id.into())
}

async fn store_dht_data(State(state): State<ApiState>, Json(payload): Json<StoreInjectData<DefaultLHTInput>>) -> Result<Json<StoreRspData>, Json<ApiErr>> {
    // todo factor out essentials to reduce code duplication
    let (tx, mut rx) = mpsc::unbounded_channel();

    let event = UseCaseEvent::InjectMessage(
        Nonce::random(),
        InjectionMessageData::Store(payload, tx),
    );

    state.sender.send((event, None)).await.map_err(|_| Json(ApiErr::SendError))?;
    let injection_result = rx.recv().await.ok_or(Json(ApiErr::ReceiveError))?; // todo timeout
    match injection_result {
        InjectionResult::Answered((ProtocolMessage::StoreRsp(payload), _)) => Ok(Json(payload.data)),
        InjectionResult::Isolated => Err(Json(ApiErr::Isolated)),
        InjectionResult::SendFailed(_) => Err(Json(ApiErr::SendError)),
        InjectionResult::Answered(_) => Err(Json(ApiErr::MessageReceiveMissmatch))
    }
}

async fn store_dht_data_example(_: State<ApiState>) -> Json<StoreInjectData<DefaultLHTInput>> {
    let example = StoreInjectData {
        data: StoreReqData { handle: NodeId::random(), data: Arc::from([1,2,4,8,16]) },
        restore: false,
    };

    Json(example)
}

async fn fetch_dht_data(State(state): State<ApiState>, Json(payload): Json<FetchReqData>) -> Result<Json<FetchRspData<DefaultLHTOutput>>, Json<ApiErr>> {
    let (tx, mut rx) = mpsc::unbounded_channel();

    let event = UseCaseEvent::InjectMessage(
        Nonce::random(),
        InjectionMessageData::Fetch(payload, tx),
    );

    state.sender.send((event, None)).await.map_err(|_| Json(ApiErr::SendError))?;
    let injection_result = rx.recv().await.ok_or(Json(ApiErr::ReceiveError))?; // todo timeout
    match injection_result {
        InjectionResult::Answered((ProtocolMessage::FetchRsp(payload), _)) => Ok(Json(payload.data)),
        InjectionResult::Isolated => Err(Json(ApiErr::Isolated)),
        InjectionResult::SendFailed(_) => Err(Json(ApiErr::SendError)),
        InjectionResult::Answered(_) => Err(Json(ApiErr::MessageReceiveMissmatch))
    }
}

async fn fetch_dht_data_example(_: State<ApiState>) -> Json<FetchReqData> {
    let example = FetchReqData {
        handle: NodeId::random(),
    };

    Json(example)
}
