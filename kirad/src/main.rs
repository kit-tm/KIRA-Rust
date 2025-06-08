use std::collections::HashSet;
use std::ffi::OsString;
use std::num::NonZeroU32;

use clap::Parser;
use futures::StreamExt;

use kira_lib::format::ProtocolMessageFormat;
use kira_lib::io::udp::async_channel;
use kira_lib::underlay::observe_underlay;
use kira_lib::Kira;
use kira_r2kad::{context::SyncContext, domain::NodeId, R2Kad};

use kira_forwarding::tables::native_tables::NativeFwdTables;

use signal_hook::consts::{SIGHUP, SIGINT, SIGKILL, SIGPIPE, SIGQUIT, SIGTERM};
use signal_hook_tokio::Signals;

#[cfg(feature = "small_buckets")]
const BUCKET_SIZE: usize = 3;
#[cfg(not(feature = "small_buckets"))]
const BUCKET_SIZE: usize = 20;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(
        short,
        long,
        value_parser,
        env = "SOCKET_PORT",
        default_value = "19219"
    )]
    socket_port: u16,
    #[clap(short, long, value_parser, env = "NODE_ID")]
    root_id: Option<NodeId>,
    /// The path to print the benchmarks after execution.
    ///
    /// This also enables benchmarking mode which disables logging.
    #[clap(short, long, value_parser, env = "BENCH_PATH")]
    benchmark_path: Option<String>,
    #[clap(
        short,
        long,
        value_parser,
        env = "NFTABLES_CONF",
        default_value = "nftables.conf"
    )]
    nftables_conf: OsString,
    #[clap(short, long, value_parser, value_delimiter = ',')]
    excluded_interfaces: Option<Vec<u32>>,
    /// Enable [tokio console](https://github.com/tokio-rs/console/tree/main/tokio-console) support.
    ///
    /// This will enable the [console_subscriber] layer
    #[cfg(feature = "tokio-console")]
    #[cfg_attr(feature = "tokio-console", clap(long))]
    tokio_console: bool,

    /// Output structured JSON log instead
    #[clap(long, env = "RUST_JSON")]
    json: bool,

    /// Enable [OpenTelemetry] trace exports.
    ///
    /// This allows you to view traces with [Jaeger].
    /// or send it to an intermediate collector supporting OTLP collection.
    /// In the future this will also publish metrics and logs using OTLP.
    ///
    /// Currently traces only span the local node, so no distributed tracing
    /// is currently implemented.
    ///
    /// # Configuration
    ///
    /// To configure the OTLP exporter you can use environment variables
    /// which are explained in further detail at [OTLP Exporter Configuration].
    /// The data is always exported using gRPC.
    ///
    /// To change the endpoint you can use `OTEL_EXPORTER_OTLP_ENDPOINT`.
    ///
    /// [OpenTelemetry]: (https://opentelemetry.io/docs/what-is-opentelemetry/)
    /// [Jaeger]: (https://www.jaegertracing.io/)
    /// [OTLP Exporter Configuration]: (https://opentelemetry.io/docs/languages/sdk-configuration/otlp-exporter/)
    #[cfg(feature = "otel")]
    #[cfg_attr(feature = "otel", clap(long, env = "RUST_OTEL"))]
    open_telemetry: bool,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let root_id: NodeId = args.root_id.unwrap_or_else(NodeId::random);

    // Setup tracing environment
    {
        use tracing::Level;
        use tracing_subscriber::{
            filter::{filter_fn, EnvFilter, FilterExt, LevelFilter, Targets},
            layer::SubscriberExt,
            util::SubscriberInitExt,
            Layer,
        };

        // disable noisy netlink_proto debug messages
        // https://github.com/rust-netlink/netlink-proto/issues/19
        let netlink_proto_filter = filter_fn(|metadata| {
            !metadata.target().starts_with("netlink_proto") || metadata.level() != &Level::DEBUG
        });

        let env_filter = EnvFilter::from_default_env().and(netlink_proto_filter.clone());
        // default output layer always present
        let reg = tracing_subscriber::registry().with(
            if args.json {
                tracing_subscriber::fmt::layer()
                    .json()
                    .flatten_event(true)
                    .boxed()
            } else {
                tracing_subscriber::fmt::layer().compact().boxed()
            }
            .with_filter(env_filter),
        );

        #[cfg(feature = "tokio-console")]
        let reg = reg.with(if args.tokio_console {
            Some(
                console_subscriber::spawn().with_filter(
                    // enable required targets for console layer
                    Targets::new()
                        .with_target("tokio", Level::TRACE)
                        .with_target("runtime", Level::TRACE)
                        .with_default(LevelFilter::OFF),
                ),
            )
        } else {
            None
        });

        #[cfg(feature = "otel")]
        let reg = reg.with(if args.open_telemetry {
            use opentelemetry::{trace::TracerProvider, KeyValue};
            use opentelemetry_sdk::{resource::Resource, trace::SdkTracerProvider};
            use opentelemetry_semantic_conventions::{attribute::SERVICE_VERSION, SCHEMA_URL};

            let resource = Resource::builder()
                .with_service_name(env!("CARGO_PKG_NAME"))
                .with_schema_url(
                    [
                        KeyValue::new("kira.node-id", format!("{root_id}")),
                        KeyValue::new(SERVICE_VERSION, env!("CARGO_PKG_VERSION")),
                    ],
                    SCHEMA_URL,
                )
                .build();
            let exporter = opentelemetry_otlp::SpanExporter::builder()
                .with_tonic()
                .build()
                .unwrap();

            let tracer_provider = SdkTracerProvider::builder()
                .with_resource(resource)
                .with_batch_exporter(exporter)
                .build();
            let tracer = tracer_provider.tracer("kirad");
            let env_filter = EnvFilter::from_default_env().and(netlink_proto_filter.clone());

            Some(
                tracing_opentelemetry::layer()
                    .with_tracer(tracer)
                    .with_filter(env_filter),
            )
        } else {
            None
        });

        reg.init();
    }

    tracing::info!(%root_id, "Starting node...");

    let excluded_interfaces = args.excluded_interfaces.map_or_else(
        // DEFAULT: ignore loopback
        || HashSet::from([NonZeroU32::new(1).unwrap().into()]),
        |vec| {
            HashSet::from_iter(
                vec.into_iter()
                    .map(|id| NonZeroU32::new(id).expect("Valid InterfaceId > 0").into()),
            )
        },
    );

    // start underlay observation
    let (connection, handle, mut underlay_updates, netlink_handle) =
        observe_underlay(excluded_interfaces.clone())
            .expect("observing underlay neighborhood failed");
    tokio::task::Builder::new()
        .name("Underlay Connection")
        .spawn(connection)
        .unwrap();

    // duplicate mpsc underlay updates channel for fwd_tables and R²/KAD
    let (tx1, underlay_updates1) = futures::channel::mpsc::unbounded();
    let (tx2, underlay_updates2) = futures::channel::mpsc::unbounded();
    tokio::task::Builder::new()
        .name("fork underlay updates")
        .spawn(async move {
            while let Some(update) = underlay_updates.next().await {
                if tx1.unbounded_send(update.clone()).is_err()
                    || tx2.unbounded_send(update).is_err()
                {
                    break;
                }
            }

            log::debug!("fork underlay updates finished");
        })
        .unwrap();

    let (fwd_tables, attach_ips) = NativeFwdTables::new(
        root_id,
        args.nftables_conf,
        netlink_handle,
        handle.clone(),
        underlay_updates1,
    )
    .await;

    tokio::task::Builder::new()
        .name("attach NodeIds to interfaces")
        .spawn(attach_ips)
        .unwrap();

    // create message sender and receiver
    let (pm_sender, pm_receiver) = async_channel(
        args.socket_port,
        ProtocolMessageFormat::MessagePack,
        handle,
        excluded_interfaces,
        root_id,
    )
    .await
    .expect("socket creation failed");
    let addr = pm_sender.local_addr().expect("failed to get bind addr");
    tracing::info!(socket_address = %addr, "Bound to socket");

    let r2kad = R2Kad::<SyncContext<_, _, _, _>, BUCKET_SIZE>::builder()
        .root_id(root_id)
        .build();

    let kira = Kira::with_components(r2kad, fwd_tables, underlay_updates2, pm_receiver, pm_sender);
    let kira = tokio::task::Builder::new()
        .name("KIRA main loop")
        .spawn(kira.start())
        .unwrap();

    log::debug!("Waiting for stop signal");
    let mut signals: Signals = Signals::new([SIGHUP, SIGTERM, SIGINT, SIGQUIT, SIGPIPE])
        .expect("failed to create signals");

    let signal = tokio::task::Builder::new()
        .name("Signal handler")
        .spawn(async move {
            while let Some(signal) = signals.next().await {
                match signal {
                    SIGHUP => println!("Received SIGHUP"),
                    SIGTERM => println!("Received SIGTERM"),
                    SIGINT => println!("Received SIGQUIT"),
                    SIGPIPE => println!("Received SIGPIPE"),
                    SIGKILL => println!("Received SIGKILL"),
                    signal => {
                        log::debug!("Received unsupported signal: {}", signal);
                        continue;
                    }
                }
                return;
            }
        })
        .unwrap();

    // we only await KIRA and not other tasks spawned
    // since KIRA should "finish" if task was crucial for its operation
    //   e. g.: API endpoint failure is not crucial for KIRA operation
    tokio::select! { _ = kira => {}, _ = signal => {}}
}
