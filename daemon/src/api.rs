use std::collections::HashMap;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::{Json, Router};
use axum::body::Bytes;
use axum::routing::{get, post};

use tokio::time::{Instant, timeout};
use tokio::sync::mpsc;

use r2kad_lib::domain::NodeId;
use r2kad_lib::domain::api;
use r2kad_lib::domain::api::{ApiErr, ApiFormatErr, DEFAULT_TIMEOUT};
use r2kad_lib::use_cases::inject_messages::InjectionResult;
use r2kad_lib::use_cases::{FetchInjectData, InjectionMessageData, StoreInjectData, UseCaseEvent};
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
        .route("/dht", post(store_dht_data).get(fetch_dht_data))
        .route("/dht/store", post(store_dht_data))
        .route("/dht/fetch", get(fetch_dht_data))
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

async fn get_node_id(State(state): State<ApiState>) -> Json<api::NodeId> {
    Json(state.node_id.into())
}

async fn store_dht_data(State(state): State<ApiState>, Query(mut params): Query<HashMap<String, String>>, body: Bytes) -> Result<Json<api::StoreRsp>, Json<ApiErr>> {
    // todo factor out essentials to reduce code duplication
    // todo move `StoreErr` into ApiErr
    let args = api::StoreArgs {
        handle: params
            .remove("handle")
            .ok_or_else(|| Json(ApiFormatErr::MissingParam("handle".to_string()).into()))?,
        restore: params
            .remove("restore")
            .as_deref()
            .map(bool::from_str)
            .unwrap_or(Ok(false))
            .map_err(|_| Json(ApiFormatErr::BoolFormatError.into()))?,
        data: Arc::from(body.as_ref()),
    };
    let (tx, mut rx) = mpsc::unbounded_channel();

    let payload = args.try_into().map_err(|_| Json(ApiFormatErr::HexFormatError.into()))?;
    let event = UseCaseEvent::InjectMessage(
        Nonce::random(),
        InjectionMessageData::Store(payload, tx),
    );

    state.sender.send((event, None)).await.map_err(|_| Json(ApiErr::SendError))?;
    let injection_result = timeout(DEFAULT_TIMEOUT, rx.recv())
        .await
        .map(|received| received.ok_or(Json(ApiErr::ReceiveError)))
        .map_err(|_| Json(ApiErr::Timeout))??;
    log::trace!(target: "api_backend", "Received injection result [{:?}]", injection_result);

    match injection_result {
        InjectionResult::Answered((ProtocolMessage::StoreRsp(payload), _)) => Ok(Json(payload.data.into())),
        InjectionResult::Isolated => Err(Json(ApiErr::Isolated)),
        InjectionResult::SendFailed(_) => Err(Json(ApiErr::SendError)),
        InjectionResult::Answered(_) => Err(Json(ApiErr::MessageReceiveMismatch))
    }
}

async fn fetch_dht_data(State(state): State<ApiState>, Query(mut params): Query<HashMap<String, String>>) -> Result<Json<api::FetchRsp>, Json<ApiErr>> {
    // todo move `FetchErr` into ApiErr
    let args = api::FetchArgs {
        handle: params
            .remove("handle")
            .ok_or_else(|| Json(ApiFormatErr::MissingParam("handle".to_string()).into()))?,
    };
    let (tx, mut rx) = mpsc::unbounded_channel();

    let event = UseCaseEvent::InjectMessage(
        Nonce::random(),
        InjectionMessageData::Fetch(args.try_into().map_err(|_| Json(ApiFormatErr::HexFormatError.into()))?, tx),
    );

    state.sender.send((event, None)).await.map_err(|_| Json(ApiErr::SendError))?;
    let injection_result = timeout(DEFAULT_TIMEOUT, rx.recv())
        .await
        .map(|received| received.ok_or(Json(ApiErr::ReceiveError)))
        .map_err(|_| Json(ApiErr::Timeout))??;
    log::trace!(target: "api_backend", "Received injection result [{:?}]", injection_result);

    match injection_result {
        InjectionResult::Answered((ProtocolMessage::FetchRsp(payload), _)) => Ok(Json(payload.data.into())),
        InjectionResult::Isolated => Err(Json(ApiErr::Isolated)),
        InjectionResult::SendFailed(_) => Err(Json(ApiErr::SendError)),
        InjectionResult::Answered(_) => Err(Json(ApiErr::MessageReceiveMismatch))
    }
}
