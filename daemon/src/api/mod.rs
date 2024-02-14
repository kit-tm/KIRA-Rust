//! REST-Server implementation

pub(crate) mod domain;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::body::Bytes;
use axum::{Json, Router};
use axum::routing::{get, post};

#[cfg(feature = "swagger_doc")]
use utoipa::OpenApi;
#[cfg(feature = "swagger_doc")]
use utoipa_swagger_ui::SwaggerUi;

use tokio::time::{Instant, timeout};
use tokio::sync::mpsc;
use domain::dht::DHTErr;

use r2kad_lib::use_cases::inject_messages::InjectionResult;
use r2kad_lib::use_cases::{ApiEvent, InjectionMessageData, UseCaseEvent};
use r2kad_lib::messaging::{Nonce, ProtocolMessage};

/// Starts a REST API-server based on the provided [ApiConfig].
///
/// The routes are defined as follows:
///   - GET `/node-id`: Return the NodeID of the running Node.
///   - POST `/dht`: Alias for `/dht/store`.
///   - GET `/dht`: Alias for `/dht/fetch`.
///   - POST `/dht/store`: Store DHT data to the network.
///   - GET `/dht/fetch`: Fetch DTH data from the network.
///   - GET `/dht/_dev/local-hashtable`: Dumps the complete hashtable
///
/// The routes are documented with the [OpenAPI Specification](https://swagger.io/resources/open-api/).
/// To view this documentation use the route `/swagger-ui/`.
///
/// # Arguments
///
/// * `api_config` - The [ApiConfig] containing the server configuration.
pub async fn start_http_server(api_config: ApiConfig) {
    log::info!("Starting api server on {}...", api_config.address);

    let api_state = ApiState {
        node_id: api_config.node_id,
        sender: api_config.sender,
    };

    #[cfg(feature = "swagger_doc")]
    let swagger = SwaggerUi::new("/swagger-ui")
        .url("/api-docs/openapi.json", ApiDoc::openapi());

    let app: Router<ApiState> = Router::new();

    #[cfg(feature = "swagger_doc")]
    let app = app.merge(swagger);

    let app = app.route("/node-id", get(get_node_id))
        .route("/dht", post(store_dht_data).get(fetch_dht_data))
        .route("/dht/store", post(store_dht_data))
        .route("/dht/fetch", get(fetch_dht_data))
        .route("/dht/_dev/local-hashtable", get(dump_local_hashtable))
        .with_state(api_state);

    let listener = tokio::net::TcpListener::bind(&api_config.address).await.unwrap();
    axum::serve(listener, app.into_make_service())
        .await
        .unwrap()
}

#[cfg(feature = "swagger_doc")]
#[derive(OpenApi)]
#[openapi(
    paths(
        get_node_id,
        store_dht_data,
        fetch_dht_data,
        dump_local_hashtable
    ),
    tags(
        (name = "", description = "")
    )
)]
struct ApiDoc;

#[derive(Clone)]
struct ApiState {
    node_id: r2kad_lib::domain::NodeId,
    sender: mpsc::Sender<(UseCaseEvent, Option<Instant>)>,
}


/// The configuration for the API-Server used by [start_http_server].
pub struct ApiConfig {
    address: SocketAddr,
    node_id: r2kad_lib::domain::NodeId,
    sender: mpsc::Sender<(UseCaseEvent, Option<Instant>)>,
}

impl ApiConfig {
    /// Creates a new `ApiConfig` instance.
    ///
    /// # Arguments
    ///
    /// * `address` - The [SocketAddr] to bind the server to.
    /// * `node_id` - The [NodeId](r2kad_lib::domain::NodeId) of the Node running the API-Server.
    /// * `sender` - The [mpsc::Sender] used to send created [UseCaseEvent]s by the server.
    ///
    /// # Returns
    ///
    /// A new [ApiConfig] instance.
    pub(crate) fn new(address: SocketAddr, node_id: r2kad_lib::domain::NodeId, sender: mpsc::Sender<(UseCaseEvent, Option<Instant>)>) -> ApiConfig {
        ApiConfig {
            address,
            node_id,
            sender,
        }
    }
}

#[cfg_attr(feature = "swagger_doc", utoipa::path(
    get,
    path = "/node-id",
    responses(
        (
            status = 200,
            body = inline(domain::NodeId),
            example = json!(domain::NodeId{node_id: "35986033727C2B8E1A10AA488216".to_string()})
        )
    )
))]
async fn get_node_id(State(state): State<ApiState>) -> Json<domain::NodeId> {
    Json(state.node_id.into())
}

fn extract_dht_handle(params: &mut HashMap<String, String>) -> Result<domain::dht::Handle, DHTErr> {
    let handle = params.remove("handle");
    let reference = params.remove("reference");

    match (handle, reference) {
        (Some(_), Some(_)) => Err(crate::api::domain::dht::ApiFormatErr::AmbiguousParams.into()),
        (Some(handle), _) => Ok(crate::api::domain::dht::Handle::Handle(crate::api::domain::NodeId { node_id: handle })),
        (_, Some(reference)) => Ok(crate::api::domain::dht::Handle::Reference(reference)),
        (None, None) => Err(crate::api::domain::dht::ApiFormatErr::MissingParams(vec![
            "handle".to_string(),
            "reference".to_string(),
        ]).into())
    }
}

#[cfg_attr(feature = "swagger_doc", utoipa::path(
    post,
    path = "/dht",
    responses(domain::dht::StoreOK, DHTErr),
    params(
        (
            "handle" = Option<String>,
            Query,
            deprecated,
            pattern = "^[A-Fa-f0-9]{28}$",
            description = "A NodeID encoded as a HexString used as Handle directly.",
            example = json ! ("35986033727C2B8E1A10AA488216")
        ),
        (
            "reference" = Option<String>,
            Query,
            description = "An arbitrary String hashed and then used as a handle.",
            example = json ! ("Foo123")
        )
    )
))]
async fn store_dht_data(State(state): State<ApiState>, Query(mut params): Query<HashMap<String, String>>, body: Bytes) -> Result<domain::dht::StoreOK, DHTErr> {
    // todo factor out essentials to reduce code duplication
    let args = crate::api::domain::dht::StoreArgs {
        handle: extract_dht_handle(&mut params)?,
        restore: params
            .remove("restore")
            .as_deref().map(str::to_lowercase).as_deref()
            .map(bool::from_str)
            .unwrap_or(Ok(false))
            .map_err(|_| crate::api::domain::dht::ApiFormatErr::BoolFormatError)?,
        data: Arc::from(body.as_ref()),
    };
    let (tx, mut rx) = mpsc::unbounded_channel();

    let payload = args.try_into().map_err(|_| crate::api::domain::dht::ApiFormatErr::HexFormatError)?;
    let event = UseCaseEvent::InjectMessage(
        None,
        InjectionMessageData::Store(payload, tx),
    );

    state.sender.send((event, None)).await.map_err(|_| DHTErr::SendError)?;
    let injection_result = timeout(domain::dht::DEFAULT_TIMEOUT, rx.recv())
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
#[cfg_attr(feature = "swagger_doc", utoipa::path(
    get,
    path = "/dht",
    responses(domain::dht::FetchRsp, DHTErr),
    params(
        (
            "handle" = Option < String >,
            Query,
            deprecated,
            pattern = "^[A-Fa-f0-9]{28}$",
            description = "A NodeID encoded as a HexString used as Handle directly.",
            example = json ! ("35986033727C2B8E1A10AA488216")
        ),
        (
            "reference" = Option < String >,
            Query,
            description = "An arbitrary String hashed and then used as a handle.",
            example = json ! ("Foo123")
        )
    )
))]
async fn fetch_dht_data(State(state): State<crate::api::ApiState>, Query(mut params): Query<HashMap<String, String>>) -> Result<Json<domain::dht::FetchRsp>, DHTErr> {
    let args = crate::api::domain::dht::FetchArgs {
        handle: extract_dht_handle(&mut params)?,
    };
    let (tx, mut rx) = mpsc::unbounded_channel();

    let event = UseCaseEvent::InjectMessage(
        None,
        InjectionMessageData::Fetch(args.try_into().map_err(|_| domain::dht::ApiFormatErr::HexFormatError)?, tx),
    );

    state.sender.send((event, None)).await.map_err(|_| DHTErr::SendError)?;
    let injection_result = timeout(domain::dht::DEFAULT_TIMEOUT, rx.recv())
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

#[cfg_attr(feature = "swagger_doc", utoipa::path(
    get,
    path = "/dht/_dev/local-hashtable",
    responses(
        (
            status = 200,
            body = inline(domain::dht::LocalHashTable),
            example = json ! ([("35986033727C2B8E1A10AA488216", ["SGVsbG8gV29ybGQ", "SSdtIGEgdGVhcG90"])].into_iter().collect::< HashMap < _, _ >> ()),
            description = "Dumps the whole local hash table by encoding the handle as a HexString and the data as Base64."
        )
    )
))]
async fn dump_local_hashtable(State(state): State<crate::api::ApiState>) -> Result<Json<domain::dht::LocalHashTable>, DHTErr> {
    let (tx, mut rx) = mpsc::unbounded_channel();

    let event = UseCaseEvent::API(ApiEvent::LocalHashTable(tx));

    state.sender.send((event, None)).await.map_err(|_| DHTErr::SendError)?;

    let local_ht = timeout(domain::dht::DEFAULT_TIMEOUT, rx.recv())
        .await
        .map(|received| received.ok_or(DHTErr::ReceiveError))
        .map_err(|_| DHTErr::Timeout)???;

    let local_ht = local_ht.into_iter()
        .map(|(handle, data)| {
            let handle = crate::api::domain::NodeId::from(handle).node_id;
            let data: Vec<String> = data.into_iter().map(hex::encode).collect();

            (handle, data)
        }).collect();

    Ok(Json(domain::dht::LocalHashTable(local_ht)))
}

