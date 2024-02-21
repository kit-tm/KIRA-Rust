use std::collections::HashMap;
use std::env;
use std::fs::{create_dir_all, File, OpenOptions};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use clap::Parser;
use opentelemetry_otlp::WithExportConfig;
use r2kad_lib::forwarding::native_tables::NativeFwdTables;
use tokio::sync::{mpsc, RwLock};
use tracing_subscriber::prelude::*;

use r2kad_daemon_lib::{Node, NodeConfig};
use r2kad_lib::domain::NodeId;
use r2kad_lib::messaging::format::ProtocolMessageFormat;
use r2kad_lib::messaging::sync_wrapper::SyncWrapper;
use r2kad_lib::messaging::{AsyncProtocolMessageReceiver, PNetInterfaceMonitor};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(short, long, value_parser, env = "SOCKET_PORT", default_value = "8080")]
    socket_port: u16,
    #[clap(short, long, value_parser, env = "NODE_ID")]
    root_id: Option<NodeId>,
    /// The path to print the benchmarks after execution.
    ///
    /// This also enables benchmarking mode which disables logging.
    #[clap(short, long, value_parser, env = "BENCH_PATH")]
    benchmark_path: Option<String>,
    #[clap(short, long, value_parser, env = "OTEL_EXPORTER_OTLP_ENDPOINT")]
    otlp_endpoint: Option<String>,
}
fn main() {

    // Setup the single threaded async runtime
    let runtime = Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime"),
    );


    let args = Args::parse();

    runtime.block_on(tracing_setup(&args.otlp_endpoint));

    let root_id: NodeId = NodeId::random();

    // Initialize benchmark file
    let benchmark_writer: Option<BufWriter<File>> = args
        .benchmark_path
        .filter(|path| !path.trim().is_empty())
        .and_then(|path| {
            let timestamp = format!("{}_{}.csv", chrono::Utc::now().to_rfc3339(), &root_id);
            let path = PathBuf::from_str(path.as_ref())
                .expect("invalid benchmark path")
                .join(timestamp);
            println!("Opening benchmark file {}", path.to_string_lossy());
            if let Some(parent_dir) = path.parent() {
                if !parent_dir.exists() {
                    create_dir_all(parent_dir).expect("failed to create directories for benchmark");
                }
            }
            OpenOptions::new()
                .write(true)
                .truncate(false)
                .create(true)
                .open(Path::new(&path))
                .map(Some)
                .unwrap_or_else(|e| {
                    log::error!("Failed to open benchmarking file: {}", e);
                    None
                })
        })
        .map(BufWriter::new);

    let mapper = PNetInterfaceMonitor::new();
    mapper.blocking_refresh();

    let fwd_table = NativeFwdTables::new();

    let ip_cache = Arc::new(RwLock::new(HashMap::new()));
    let channel = r2kad_lib::messaging::udp::async_channel(
        args.socket_port,
        ip_cache,
        mapper,
        ProtocolMessageFormat::MessagePack,
    );
    let (message_sender, message_receiver) = runtime
        .block_on(channel)
        .expect("failed to initialize IO channel");

    // Due to the behaviour of the UDP Sender and Receiver there is no need to handle hardware events.
    // The Network Stack will handle new interfaces coming up and going down.
    // Node failure will be detected through periodic path probing

    let addr = message_sender
        .local_addr()
        .expect("failed to get bind addr");
    println!("Using Address: {}", addr);

    let (pmr_sender, pmr_receiver) = mpsc::channel(1);
    let boxed_receiver: Box<dyn AsyncProtocolMessageReceiver + Send> = Box::new(message_receiver);
    if let Err(e) = pmr_sender.blocking_send(boxed_receiver) {
        log::error!("Failed to send protocol message receiver to node: {}", e);
        return;
    }

    let config = NodeConfig {
        benchmark_path: benchmark_writer,
        ..NodeConfig::default()
    };

    let node = Node::new(
        config,
        root_id,
        Arc::clone(&runtime),
        pmr_receiver,
        SyncWrapper::new(message_sender, Arc::clone(&runtime)),
        fwd_table,
    );

    node.blocking_start();
}

async fn tracing_setup(otlp_endpoint: &Option<String>) {
    let fmt_layer = tracing_subscriber::fmt::layer();

    if let Some(endpoint) = otlp_endpoint {
        let exporter = opentelemetry_otlp::new_exporter()
            .tonic()
            .with_endpoint(endpoint);

        let tracer = opentelemetry_otlp::new_pipeline()
            .tracing()
            .with_exporter(exporter)
            .install_batch(opentelemetry_sdk::runtime::Tokio)
            .expect("Couldn't create tracer");

        tracing_subscriber::registry()
            .with(fmt_layer)
            .with(tracing_subscriber::filter::EnvFilter::from_default_env())
            .with(tracing_opentelemetry::layer().with_tracer(tracer))
            .init();
    } else {
        tracing_subscriber::registry()
            .with(fmt_layer)
            .with(tracing_subscriber::filter::EnvFilter::from_default_env())
            .init();
    }



    tracing::warn!("Started tracing");
}
