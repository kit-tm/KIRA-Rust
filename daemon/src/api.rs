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
use r2kad_lib::use_cases::{ApiEvent, InjectionMessageData, UseCaseEvent};
use r2kad_lib::messaging::dht::{DefaultLHTOutput};
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
        .route("/dht/_dev/local-hashtable", get(dump_local_hashtable))
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

async fn store_dht_data(State(state): State<ApiState>, Query(mut params): Query<HashMap<String, String>>, body: Bytes) -> Result<api::StoreOK, api::ApiErr> {
    // todo factor out essentials to reduce code duplication
    // todo move `StoreErr` into ApiErr
    let args = api::StoreArgs {
        handle: params
            .remove("handle")
            .ok_or_else(|| ApiFormatErr::MissingParams(vec!["handle".to_owned()]))?,
        restore: params
            .remove("restore")
            .as_deref().map(str::to_lowercase).as_deref()
            .map(bool::from_str)
            .unwrap_or(Ok(false))
            .map_err(|_| ApiFormatErr::BoolFormatError)?,
        data: Arc::from(body.as_ref()),
    };
    let (tx, mut rx) = mpsc::unbounded_channel();

    let payload = args.try_into().map_err(|_| ApiFormatErr::HexFormatError)?;
    let event = UseCaseEvent::InjectMessage(
        Nonce::random(),
        InjectionMessageData::Store(payload, tx),
    );

    state.sender.send((event, None)).await.map_err(|_| ApiErr::SendError)?;
    let injection_result = timeout(DEFAULT_TIMEOUT, rx.recv())
        .await
        .map(|received| received.ok_or(ApiErr::ReceiveError))
        .map_err(|_| ApiErr::Timeout)??;
    log::trace!(target: "api_backend", "Received injection result [{:?}]", injection_result);

    match injection_result {
        InjectionResult::Answered((ProtocolMessage::StoreRsp(payload), _)) => Ok(payload.data.status?.into()),
        InjectionResult::Isolated => Err(ApiErr::Isolated),
        InjectionResult::SendFailed(_) => Err(ApiErr::SendError),
        InjectionResult::Answered(_) => Err(ApiErr::MessageReceiveMismatch)
    }
}

// todo dont use JSON for DHTOutput
async fn fetch_dht_data(State(state): State<ApiState>, Query(mut params): Query<HashMap<String, String>>) -> Result<Json<DefaultLHTOutput>, api::ApiErr> {
    let args = api::FetchArgs {
        handle: params
            .remove("handle")
            .ok_or_else(|| ApiFormatErr::MissingParams(vec!["handle".to_owned()]))?,
    };
    let (tx, mut rx) = mpsc::unbounded_channel();

    let event = UseCaseEvent::InjectMessage(
        Nonce::random(),
        InjectionMessageData::Fetch(args.try_into().map_err(|_| ApiFormatErr::HexFormatError)?, tx),
    );

    state.sender.send((event, None)).await.map_err(|_| ApiErr::SendError)?;
    let injection_result = timeout(DEFAULT_TIMEOUT, rx.recv())
        .await
        .map(|received| received.ok_or(ApiErr::ReceiveError))
        .map_err(|_| ApiErr::Timeout)??;
    log::trace!(target: "api_backend", "Received injection result [{:?}]", injection_result);

    match injection_result {
        InjectionResult::Answered((ProtocolMessage::FetchRsp(payload), _)) => Ok(Json(payload.data.data?)),
        InjectionResult::Isolated => Err(ApiErr::Isolated),
        InjectionResult::SendFailed(_) => Err(ApiErr::SendError),
        InjectionResult::Answered(_) => Err(ApiErr::MessageReceiveMismatch)
    }
}

async fn dump_local_hashtable(State(state): State<ApiState>) -> Result<Json<api::LocalHashTable>, api::ApiErr> {
    let (tx, mut rx) = mpsc::unbounded_channel();

    let event = UseCaseEvent::API(ApiEvent::LocalHashTable(tx));

    state.sender.send((event, None)).await.map_err(|_| ApiErr::SendError)?;

    let local_ht = rx.recv().await
        .map(Json)
        .ok_or(ApiErr::ReceiveError);

    return local_ht;
}
