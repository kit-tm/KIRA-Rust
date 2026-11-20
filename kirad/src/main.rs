use std::{
    collections::HashSet,
    num::NonZeroU32,
};

use clap::{
    CommandFactory,
    Parser,
    error::ErrorKind,
};
use futures::StreamExt;
#[cfg(feature = "srv6")]
use kira_forwarding::{
    netlink::remove_routes,
    tables::srv6_tables::Srv6FwdTables,
};
use kira_forwarding::{
    netlink::{
        NETLINK_NETFILTER,
        NetfilterNetlinkHandle,
        RouteNetlinkHandle,
        new_connection,
        remove_forwarding_state,
    },
    tables::native_tables::NativeFwdTables,
};
use kira_lib::{
    Kira,
    io::udp::async_channel,
    underlay::observe_underlay,
};
use kira_r2kad::{
    R2Kad,
    context::SyncContext,
    domain::{
        DEFAULT_NODE_IP_PREFIX,
        DEFAULT_PATH_IP_PREFIX,
        IpPrefixes,
        Ipv6Prefix16,
        NodeId,
        PathSegmentation,
    },
};
use kirad_lib::{
    ForwardingMode,
    R2KadMessageFormat,
};
use signal_hook::consts::{
    SIGHUP,
    SIGINT,
    SIGKILL,
    SIGPIPE,
    SIGQUIT,
    SIGTERM,
};
use signal_hook_tokio::Signals;

#[cfg(feature = "small_buckets")]
const BUCKET_SIZE: usize = 3;
#[cfg(not(feature = "small_buckets"))]
// for large networks the recommendation is 40
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
    /// Path of a JSON file to write the R²/KAD message statistics to on shutdown.
    ///
    /// It contains the sent and received messages and bytes (UDP payload and on the wire
    /// incl. IPv6/UDP headers) in total and per message kind, also as averages per second.
    #[clap(short, long, value_parser, env = "BENCH_PATH")]
    benchmark_path: Option<std::path::PathBuf>,
    /// Interval in seconds for appending snapshots of the cumulative message statistics
    /// as JSON lines to `<BENCH_PATH>.jsonl` (0 disables it).
    ///
    /// Requires `--benchmark-path`.
    #[clap(long, value_parser, env = "STATS_INTERVAL", default_value_t = 0)]
    stats_interval: u64,
    #[clap(short, long, value_parser, value_delimiter = ',')]
    excluded_interfaces: Option<Vec<u32>>,
    /// IPv6 prefix of the addresses derived from NodeIds (NodeIPs).
    ///
    /// Must be a /16 prefix and the same on all nodes of the network.
    #[clap(long, value_parser, env = "NODE_IP_PREFIX", default_value_t = DEFAULT_NODE_IP_PREFIX)]
    node_ip_prefix: Ipv6Prefix16,
    /// IPv6 prefix of the addresses derived from PathIds (PathIPs).
    ///
    /// Must be a /16 prefix that differs from the NodeIP prefix
    /// and must be the same on all nodes of the network.
    #[clap(long, value_parser, env = "PATH_IP_PREFIX", default_value_t = DEFAULT_PATH_IP_PREFIX)]
    path_ip_prefix: Ipv6Prefix16,
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

    #[arg(short, long, env = "KIRAD_FORMAT", value_enum, default_value_t)]
    format: R2KadMessageFormat,

    /// Encapsulation used by the fast forwarding of data packets.
    ///
    /// Must be the same on all nodes of the network.
    #[arg(long, env = "KIRA_FORWARDING", value_enum, default_value_t)]
    forwarding: ForwardingMode,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let root_id: NodeId = args.root_id.unwrap_or_else(NodeId::random);
    let ip_prefixes = IpPrefixes::new(args.node_ip_prefix, args.path_ip_prefix)
        .unwrap_or_else(|e| Args::command().error(ErrorKind::ArgumentConflict, e).exit());

    // Setup tracing environment
    {
        use tracing::Level;
        #[cfg(any(feature = "otel", feature = "tokio-console"))]
        use tracing_subscriber::filter::LevelFilter;
        #[cfg(feature = "tokio-console")]
        use tracing_subscriber::filter::Targets;
        use tracing_subscriber::{
            Layer,
            filter::{
                EnvFilter,
                FilterExt,
                filter_fn,
            },
            layer::SubscriberExt,
            util::SubscriberInitExt,
        };

        // disable noisy netlink_proto debug messages
        // https://github.com/rust-netlink/netlink-proto/issues/19
        let netlink_proto_filter = filter_fn(|metadata| {
            !metadata.target().starts_with("netlink_proto") || metadata.level() != &Level::DEBUG
        });

        let env_filter = EnvFilter::from_default_env().and(netlink_proto_filter.clone());
        #[cfg(feature = "otel")]
        let env_filter = if args.open_telemetry {
            // disabling debug and tracing logging if open telemetry export is active
            env_filter.and(LevelFilter::INFO).boxed()
        } else {
            env_filter.boxed()
        };

        // default output layer always present
        let reg = tracing_subscriber::registry().with(
            if args.json {
                tracing_subscriber::fmt::layer()
                    .json()
                    .flatten_event(true)
                    .boxed()
            } else {
                tracing_subscriber::fmt::layer().pretty().boxed()
            }
            .with_filter(env_filter),
        );

        // logging to tokio-console
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

        // open telemetry export layer
        #[cfg(feature = "otel")]
        let reg = reg.with(if args.open_telemetry {
            use opentelemetry::{
                KeyValue,
                trace::TracerProvider,
            };
            use opentelemetry_sdk::{
                resource::Resource,
                trace::SdkTracerProvider,
            };
            use opentelemetry_semantic_conventions::{
                SCHEMA_URL,
                attribute::SERVICE_VERSION,
            };

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

    tracing::info!(
        %root_id,
        node_ip_prefix = %ip_prefixes.node_ip_prefix(),
        path_ip_prefix = %ip_prefixes.path_ip_prefix(),
        "Starting node..."
    );

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
    let (connection, handle, mut underlay_updates, handle_rtnetlink) =
        observe_underlay(excluded_interfaces.clone())
            .expect("observing underlay neighborhood failed");
    tokio::task::Builder::new()
        .name("Underlay Connection")
        .spawn(connection)
        .unwrap();

    // duplicate mpsc underlay updates channel for fwd_tables and R²/KAD
    let (tx1, underlay_updates1) = futures::channel::mpsc::unbounded();
    let (tx2, underlay_updates2) = futures::channel::mpsc::unbounded();
    // With SRv6 forwarding, R²/KAD gets the updates from the forwarding tables after they
    // prepared the interfaces (see Srv6FwdTables::new).
    let fork_to_r2kad = match args.forwarding {
        ForwardingMode::Gre => Some(tx2.clone()),
        #[cfg(feature = "srv6")]
        ForwardingMode::Srv6 => None,
    };
    tokio::task::Builder::new()
        .name("fork underlay updates")
        .spawn(async move {
            while let Some(update) = underlay_updates.next().await {
                if fork_to_r2kad
                    .as_ref()
                    .is_some_and(|tx2| tx2.unbounded_send(update.clone()).is_err())
                    || tx1.unbounded_send(update).is_err()
                {
                    break;
                }
            }

            log::debug!("fork underlay updates finished");
        })
        .unwrap();

    // create message sender and receiver
    let (pm_sender, pm_receiver) = async_channel(
        args.socket_port,
        args.format.into(),
        handle.clone(),
        excluded_interfaces,
        root_id,
    )
    .await
    .expect("socket creation failed");
    let addr = pm_sender.local_addr().expect("failed to get bind addr");
    tracing::info!(socket_address = %addr, "Bound to socket");

    let stats = pm_sender.stats();
    let stats_format = format!("{:?}", args.format).to_lowercase();
    if let Some(path) = args.benchmark_path.as_ref()
        && args.stats_interval > 0
    {
        let mut jsonl = path.as_os_str().to_owned();
        jsonl.push(".jsonl");
        tokio::task::Builder::new()
            .name("Message statistics")
            .spawn(kirad_lib::stats::write_periodically(
                jsonl.into(),
                std::time::Duration::from_secs(args.stats_interval),
                stats.clone(),
                root_id.to_string(),
                stats_format.clone(),
            ))
            .unwrap();
    }

    // GRE carries a single PathID, SRv6 one per path segment
    let path_segmentation = match args.forwarding {
        ForwardingMode::Gre => PathSegmentation::OneSegment,
        #[cfg(feature = "srv6")]
        ForwardingMode::Srv6 => PathSegmentation::TwoSegments,
    };
    let r2kad = R2Kad::<SyncContext<_, _, _, _, _>, BUCKET_SIZE>::builder()
        .root_id(root_id)
        .path_segmentation(path_segmentation)
        .current_time(std::time::Instant::now())
        .build();

    tracing::info!(forwarding = ?args.forwarding, "Setting up fast forwarding");
    // The forwarding tables and what is needed to remove the forwarding state on shutdown
    let (mut kira, cleanup) = match args.forwarding {
        ForwardingMode::Gre => {
            let (connection_nfnetlink, handle_nfnetlink, _) =
                new_connection(NETLINK_NETFILTER).unwrap();
            tokio::task::Builder::new()
                .name("Netfilter Connection")
                .spawn(connection_nfnetlink)
                .unwrap();

            let cleanup = ForwardingCleanup::Gre {
                handle_rtnetlink: handle_rtnetlink.clone(),
                handle_nfnetlink: handle_nfnetlink.clone(),
            };
            let (fwd_tables, attach_ips) = NativeFwdTables::new(
                root_id,
                ip_prefixes,
                handle_rtnetlink,
                handle_nfnetlink,
                handle,
                underlay_updates1,
            )
            .await;

            tokio::task::Builder::new()
                .name("attach NodeIds to interfaces")
                .spawn(attach_ips)
                .unwrap();

            let kira =
                Kira::with_components(r2kad, fwd_tables, underlay_updates2, pm_receiver, pm_sender);
            let kira = tokio::task::Builder::new()
                .name("KIRA main loop")
                .spawn(kira.start())
                .unwrap();
            (kira, cleanup)
        }
        #[cfg(feature = "srv6")]
        ForwardingMode::Srv6 => {
            // a previous instance using GRE may have left its state behind (best effort)
            if let Ok((connection_nfnetlink, handle_nfnetlink, _)) =
                new_connection(NETLINK_NETFILTER)
            {
                let connection = tokio::spawn(connection_nfnetlink);
                if let Err(e) =
                    remove_forwarding_state(&handle_rtnetlink, &handle_nfnetlink, &ip_prefixes)
                        .await
                {
                    log::debug!("Removing forwarding state of GRE forwarding failed: {e}");
                }
                connection.abort();
            }

            let (fwd_tables, attach) = Srv6FwdTables::new(
                root_id,
                ip_prefixes,
                handle_rtnetlink.clone(),
                handle,
                underlay_updates1,
                tx2,
            )
            .await
            .expect("SRv6 forwarding should successfully be set up");

            // the eBPF program is detached from the interfaces when this task ends
            let attach = tokio::task::Builder::new()
                .name("attach NodeIds and eBPF program to interfaces")
                .spawn(attach)
                .unwrap();
            let cleanup = ForwardingCleanup::Srv6 {
                handle_rtnetlink,
                attach,
            };

            let kira =
                Kira::with_components(r2kad, fwd_tables, underlay_updates2, pm_receiver, pm_sender);
            let kira = tokio::task::Builder::new()
                .name("KIRA main loop")
                .spawn(kira.start())
                .unwrap();
            (kira, cleanup)
        }
    };

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
                        log::debug!("Received unsupported signal: {signal}");
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
    tokio::select! { _ = &mut kira => {}, _ = signal => {}}
    // no more forwarding updates during the cleanup
    kira.abort();

    if let Some(path) = args.benchmark_path.as_ref() {
        match kirad_lib::stats::write_summary(path, &stats, &root_id.to_string(), &stats_format) {
            Ok(()) => tracing::info!(path = %path.display(), "Wrote message statistics"),
            Err(err) => {
                tracing::error!(path = %path.display(), %err, "Writing message statistics failed")
            }
        }
    }

    // leave no stale forwarding state behind
    match cleanup {
        ForwardingCleanup::Gre {
            handle_rtnetlink,
            handle_nfnetlink,
        } => {
            if let Err(e) =
                remove_forwarding_state(&handle_rtnetlink, &handle_nfnetlink, &ip_prefixes).await
            {
                log::error!("Removing the forwarding state failed: {e}");
            }
        }
        #[cfg(feature = "srv6")]
        ForwardingCleanup::Srv6 {
            handle_rtnetlink,
            attach,
        } => {
            // detaches the eBPF program from all interfaces
            attach.abort();
            let _ = attach.await;
            if let Err(e) = remove_routes(&handle_rtnetlink, &ip_prefixes).await {
                log::error!("Removing the forwarding state failed: {e}");
            }
        }
    }
}

/// What is needed to remove the forwarding state on shutdown.
enum ForwardingCleanup {
    Gre {
        handle_rtnetlink: RouteNetlinkHandle,
        handle_nfnetlink: NetfilterNetlinkHandle,
    },
    #[cfg(feature = "srv6")]
    Srv6 {
        handle_rtnetlink: RouteNetlinkHandle,
        /// Task holding the eBPF program
        attach: tokio::task::JoinHandle<()>,
    },
}
