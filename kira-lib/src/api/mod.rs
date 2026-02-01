//! REST-Server implementation

pub(crate) mod domain;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};

use kira_r2kad::domain::protocol_event::DebugEvent;
#[cfg(feature = "swagger_doc")]
use utoipa::OpenApi;
#[cfg(feature = "swagger_doc")]
use utoipa_swagger_ui::SwaggerUi;

use domain::dht::DHTErr;
use tokio::sync::mpsc;
use tokio::time::timeout;

use kira_r2kad::messaging::ProtocolMessage;
use kira_r2kad::use_cases::inject_messages::InjectionResult;
use kira_r2kad::use_cases::{ApiEvent, InjectionMessageData};

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
    let swagger = SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi());

    let app: Router<ApiState> = Router::new();

    #[cfg(feature = "swagger_doc")]
    let app = app.merge(swagger);

    let app = app
        .route("/node-id", get(get_node_id))
        .route("/dht", post(store_dht_data).get(fetch_dht_data))
        .route("/dht/store", post(store_dht_data))
        .route("/dht/fetch", get(fetch_dht_data))
        .route("/dht/_dev/local-hashtable", get(dump_local_hashtable))
        .route("/_dev/uln-table", get(dump_un_table))
        .route("/_dev/routing-table", get(dump_routing_table))
        .route("/_dev/vicinity-graph", get(dump_vicinity_graph))
        .with_state(api_state);

    let listener = tokio::net::TcpListener::bind(&api_config.address)
        .await
        .unwrap();
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
    node_id: kira_r2kad::domain::NodeId,
    sender: mpsc::Sender<DebugEvent>,
}

/// The configuration for the API-Server used by [start_http_server].
pub struct ApiConfig {
    address: SocketAddr,
    node_id: kira_r2kad::domain::NodeId,
    sender: mpsc::Sender<DebugEvent>,
}

impl ApiConfig {
    /// Creates a new `ApiConfig` instance.
    ///
    /// # Arguments
    ///
    /// * `address` - The [SocketAddr] to bind the server to.
    /// * `node_id` - The [NodeId](kira_r2kad::domain::NodeId) of the Node running the API-Server.
    /// * `sender` - The [mpsc::Sender] used to send created [UseCaseEvent]s by the server.
    ///
    /// # Returns
    ///
    /// A new [ApiConfig] instance.
    pub(crate) fn new(
        address: SocketAddr,
        node_id: kira_r2kad::domain::NodeId,
        sender: mpsc::Sender<DebugEvent>,
    ) -> ApiConfig {
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
    let key = params.remove("key");

    match (handle, key) {
        (Some(_), Some(_)) => Err(crate::api::domain::dht::ApiFormatErr::AmbiguousParams.into()),
        (Some(handle), _) => Ok(crate::api::domain::dht::Handle::Handle(
            crate::api::domain::NodeId { node_id: handle },
        )),
        (_, Some(key)) => Ok(crate::api::domain::dht::Handle::Key(key)),
        (None, None) => Err(crate::api::domain::dht::ApiFormatErr::MissingParams(vec![
            "handle".to_string(),
            "key".to_string(),
        ])
        .into()),
    }
}

/// Store a value in the DHT under some key.
#[cfg_attr(feature = "swagger_doc", utoipa::path(
    post,
    path = "/dht",
    responses(domain::dht::StoreOK, DHTErr),
    request_body(content = [u8], description = "The value to store in the DHT"),
    params(
        (
            "key" = Option < String >,
            Query,
            description = "The key under which to store the value in the DHT",
            example = json ! ("Foo123")
        ))
))]
async fn store_dht_data(
    State(state): State<ApiState>,
    Query(mut params): Query<HashMap<String, String>>,
    body: Bytes,
) -> Result<domain::dht::StoreOK, DHTErr> {
    // TODO: factor out essentials to reduce code duplication
    let args = crate::api::domain::dht::StoreArgs {
        handle: extract_dht_handle(&mut params)?,
        restore: params
            .remove("restore")
            .as_deref()
            .map(str::to_lowercase)
            .as_deref()
            .map(bool::from_str)
            .unwrap_or(Ok(false))
            .map_err(|_| crate::api::domain::dht::ApiFormatErr::BoolFormatError)?,
        data: Arc::from(body.as_ref()),
    };
    let (tx, mut rx) = mpsc::unbounded_channel();

    let payload = args
        .try_into()
        .map_err(|_| crate::api::domain::dht::ApiFormatErr::HexFormatError)?;
    let event = DebugEvent::InjectMessage(None, InjectionMessageData::Store(payload, tx));

    state
        .sender
        .send(event)
        .await
        .map_err(|_| DHTErr::SendError)?;
    let injection_result = timeout(domain::dht::DEFAULT_TIMEOUT, rx.recv())
        .await
        .map(|received| received.ok_or(DHTErr::ReceiveError))
        .map_err(|_| DHTErr::Timeout)??;
    log::trace!(target: "api_backend", "Received injection result [{injection_result:?}]");

    match injection_result {
        InjectionResult::Answered(boxedtuple) => match *boxedtuple {
            (ProtocolMessage::StoreRsp(payload), _) => Ok(payload.data.status?.into()),
            _ => Err(DHTErr::MessageReceiveMismatch),
        },
        InjectionResult::Isolated => Err(DHTErr::Isolated),
    }
}

// TODO: dont use JSON for DHTOutput
/// Fetch a value stored in the DHT under some key.
#[cfg_attr(feature = "swagger_doc", utoipa::path(
    get,
    path = "/dht",
    responses(
        (status = 200, description = "The list of values stored in the DHT under that key. Encoded as base64.", example = json!(["QmFyMTIzCg=="]), body = domain::dht::FetchRsp), 
        (status = "4XX", description = "An error during DHT fetch", body = DHTErr)),
    params(
        (
            "key" = Option < String >,
            Query,
            description = "Fetches the value(s) stored in the DHT under key <key>!",
            example = json ! ("Foo123")
        ))
))]
async fn fetch_dht_data(
    State(state): State<crate::api::ApiState>,
    Query(mut params): Query<HashMap<String, String>>,
) -> Result<Json<domain::dht::FetchRsp>, DHTErr> {
    let args = crate::api::domain::dht::FetchArgs {
        handle: extract_dht_handle(&mut params)?,
    };
    let (tx, mut rx) = mpsc::unbounded_channel();

    let event = DebugEvent::InjectMessage(
        None,
        InjectionMessageData::Fetch(
            args.try_into()
                .map_err(|_| domain::dht::ApiFormatErr::HexFormatError)?,
            tx,
        ),
    );

    state
        .sender
        .send(event)
        .await
        .map_err(|_| DHTErr::SendError)?;
    let injection_result = timeout(domain::dht::DEFAULT_TIMEOUT, rx.recv())
        .await
        .map(|received| received.ok_or(DHTErr::ReceiveError))
        .map_err(|_| DHTErr::Timeout)??;
    log::trace!(target: "api_backend", "Received injection result [{injection_result:?}]");

    match injection_result {
        InjectionResult::Answered(boxedtuple) => match *boxedtuple {
            (ProtocolMessage::FetchRsp(payload), _) => Ok(Json(payload.data.data?.into())),
            _ => Err(DHTErr::MessageReceiveMismatch),
        },
        InjectionResult::Isolated => Err(DHTErr::Isolated),
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
async fn dump_local_hashtable(
    State(state): State<crate::api::ApiState>,
) -> Result<Json<domain::dht::LocalHashTable>, DHTErr> {
    let (tx, mut rx) = mpsc::unbounded_channel();

    let event = ApiEvent::LocalHashTable(tx);

    state
        .sender
        .send(event.into())
        .await
        .map_err(|_| DHTErr::SendError)?;

    let local_ht = timeout(domain::dht::DEFAULT_TIMEOUT, rx.recv())
        .await
        .map(|received| received.ok_or(DHTErr::ReceiveError))
        .map_err(|_| DHTErr::Timeout)???;

    let local_ht = local_ht
        .into_iter()
        .map(|(handle, data)| {
            let handle = crate::api::domain::NodeId::from(handle).node_id;
            let data: Vec<String> = data.into_iter().map(hex::encode).collect();

            (handle, data)
        })
        .collect();

    Ok(Json(domain::dht::LocalHashTable(local_ht)))
}

// TODO: Swagger doc
// TODO: create generic ApiErr

async fn dump_un_table(State(state): State<crate::api::ApiState>) -> Result<String, Json<DHTErr>> {
    let (tx, mut rx) = mpsc::unbounded_channel();

    let event = ApiEvent::ULNTable(tx);

    state
        .sender
        .send(event.into())
        .await
        .map_err(|_| DHTErr::SendError)?;

    let un_table_debug_string = timeout(domain::dht::DEFAULT_TIMEOUT, rx.recv())
        .await
        .map(|received| received.ok_or(DHTErr::ReceiveError))
        .map_err(|_| DHTErr::Timeout)??;

    Ok(un_table_debug_string)
}

async fn dump_routing_table(
    State(state): State<crate::api::ApiState>,
) -> Result<String, Json<DHTErr>> {
    let (tx, mut rx) = mpsc::unbounded_channel();

    let event = ApiEvent::RoutingTable(tx);

    state
        .sender
        .send(event.into())
        .await
        .map_err(|_| DHTErr::SendError)?;

    let routing_table_debug_string = timeout(domain::dht::DEFAULT_TIMEOUT, rx.recv())
        .await
        .map(|received| received.ok_or(DHTErr::ReceiveError))
        .map_err(|_| DHTErr::Timeout)??;

    Ok(routing_table_debug_string)
}

async fn dump_vicinity_graph(
    State(state): State<crate::api::ApiState>,
) -> Result<String, Json<DHTErr>> {
    let (tx, mut rx) = mpsc::unbounded_channel();

    let event = ApiEvent::VicinityGraph(tx);

    state
        .sender
        .send(event.into())
        .await
        .map_err(|_| DHTErr::SendError)?;

    let vicinity_graph_debug_string = timeout(domain::dht::DEFAULT_TIMEOUT, rx.recv())
        .await
        .map(|received| received.ok_or(DHTErr::ReceiveError))
        .map_err(|_| DHTErr::Timeout)??;

    Ok(vicinity_graph_debug_string)
}
