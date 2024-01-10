pub mod domain;

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
use domain::dht;
use domain::dht::{DHTErr, ApiFormatErr, DEFAULT_TIMEOUT};

use r2kad_lib::use_cases::inject_messages::InjectionResult;
use r2kad_lib::use_cases::{ApiEvent, InjectionMessageData, UseCaseEvent};
use r2kad_lib::messaging::{Nonce, ProtocolMessage};
use crate::api::domain::dht::{FetchRsp, LocalHashTable, StoreOK};
use crate::api::domain::NodeId;


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
    node_id: r2kad_lib::domain::NodeId,
    sender: mpsc::Sender<(UseCaseEvent, Option<Instant>)>,
}

pub struct ApiConfig {
    address: SocketAddr,
    node_id: r2kad_lib::domain::NodeId,
    sender: mpsc::Sender<(UseCaseEvent, Option<Instant>)>,
}

impl ApiConfig {
    pub(crate) fn new(address: SocketAddr, node_id: r2kad_lib::domain::NodeId, sender: mpsc::Sender<(UseCaseEvent, Option<Instant>)>) -> ApiConfig {
        ApiConfig {
            address,
            node_id,
            sender,
        }
    }
}

async fn get_node_id(State(state): State<ApiState>) -> Json<NodeId> {
    Json(state.node_id.into())
}

fn _extract_dht_handle(params: &mut HashMap<String, String>) -> Result<dht::Handle, DHTErr> {
    let handle = params.remove("handle");
    let reference = params.remove("reference");


    match (handle, reference) {
        (Some(_), Some(_)) => Err(ApiFormatErr::AmbiguousParams.into()),
        (Some(handle), _) => Ok(dht::Handle::Handle(NodeId { node_id: handle })),
        (_, Some(reference)) => Ok(dht::Handle::Reference(reference)),
        (None, None) => Err(ApiFormatErr::MissingParams(vec![
            "handle".to_string(),
            "reference".to_string(),
        ]).into())
    }
}

async fn store_dht_data(State(state): State<ApiState>, Query(mut params): Query<HashMap<String, String>>, body: Bytes) -> Result<StoreOK, DHTErr> {
    // todo factor out essentials to reduce code duplication
    let args = dht::StoreArgs {
        handle: _extract_dht_handle(&mut params)?,
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

    state.sender.send((event, None)).await.map_err(|_| DHTErr::SendError)?;
    let injection_result = timeout(DEFAULT_TIMEOUT, rx.recv())
        .await
        .map(|received| received.ok_or(DHTErr::ReceiveError))
        .map_err(|_| DHTErr::Timeout)??;
    log::trace!(target: "api_backend", "Received injection result [{:?}]", injection_result);

    match injection_result {
        InjectionResult::Answered((ProtocolMessage::StoreRsp(payload), _)) => Ok(payload.data.status?.into()),
        InjectionResult::Isolated => Err(DHTErr::Isolated),
        InjectionResult::SendFailed(_) => Err(DHTErr::SendError),
        InjectionResult::Answered(_) => Err(DHTErr::MessageReceiveMismatch)
    }
}

// todo dont use JSON for DHTOutput
async fn fetch_dht_data(State(state): State<ApiState>, Query(mut params): Query<HashMap<String, String>>) -> Result<Json<FetchRsp>, DHTErr> {
    let args = dht::FetchArgs {
        handle: _extract_dht_handle(&mut params)?,
    };
    let (tx, mut rx) = mpsc::unbounded_channel();

    let event = UseCaseEvent::InjectMessage(
        Nonce::random(),
        InjectionMessageData::Fetch(args.try_into().map_err(|_| ApiFormatErr::HexFormatError)?, tx),
    );

    state.sender.send((event, None)).await.map_err(|_| DHTErr::SendError)?;
    let injection_result = timeout(DEFAULT_TIMEOUT, rx.recv())
        .await
        .map(|received| received.ok_or(DHTErr::ReceiveError))
        .map_err(|_| DHTErr::Timeout)??;
    log::trace!(target: "api_backend", "Received injection result [{:?}]", injection_result);

    match injection_result {
        InjectionResult::Answered((ProtocolMessage::FetchRsp(payload), _)) => Ok(Json(payload.data.data?.into())),
        InjectionResult::Isolated => Err(DHTErr::Isolated),
        InjectionResult::SendFailed(_) => Err(DHTErr::SendError),
        InjectionResult::Answered(_) => Err(DHTErr::MessageReceiveMismatch)
    }
}

async fn dump_local_hashtable(State(state): State<ApiState>) -> Result<Json<LocalHashTable>, DHTErr> {
    let (tx, mut rx) = mpsc::unbounded_channel();

    let event = UseCaseEvent::API(ApiEvent::LocalHashTable(tx));

    state.sender.send((event, None)).await.map_err(|_| DHTErr::SendError)?;

    let local_ht = timeout(DEFAULT_TIMEOUT, rx.recv())
        .await
        .map(|received| received.ok_or(DHTErr::ReceiveError))
        .map_err(|_| DHTErr::Timeout)???;

    let local_ht = local_ht.into_iter()
        .map(|(handle, data)| {
            let handle = NodeId::from(handle).node_id;
            let data: Vec<String> = data.into_iter().map(hex::encode).collect();

            (handle, data)
        }).collect();

    Ok(Json(LocalHashTable(local_ht)))
}
