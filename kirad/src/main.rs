use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs::{create_dir_all, File, OpenOptions};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use clap::Parser;
use kira_lib::forwarding::native_tables::NativeFwdTables;
use kira_lib::forwarding::platform;
use kira_lib::hardware_events::{HardwareEvent, HardwareEventRegistry};
use tokio::sync::{mpsc, RwLock};

use kira_lib::domain::NodeId;
use kira_lib::messaging::format::ProtocolMessageFormat;
use kira_lib::messaging::sync_wrapper::SyncWrapper;
use kira_lib::messaging::{AsyncProtocolMessageReceiver, PNetInterfaceMonitor};
use kirad_lib::{Node, NodeConfig};
use tracing_subscriber::prelude::*;

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
}

fn main() {
    // Setup tracing environment
    let fmt_layer = tracing_subscriber::fmt::layer().with_ansi(false);
    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(tracing_subscriber::filter::EnvFilter::from_default_env())
        .init();

    // Setup the single threaded async runtime
    let runtime = Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime"),
    );

    let args = Args::parse();

    let root_id: NodeId = args.root_id.unwrap_or_else(NodeId::random);
    tracing::info!(%root_id, "Starting node...");

    let excluded_interfaces = args.excluded_interfaces.map_or_else(
        || HashSet::default(),
        |vec| HashSet::from_iter(vec.into_iter()),
    );

    let mapper = PNetInterfaceMonitor::with_excluded_interfaces(excluded_interfaces.clone());
    // attach root-id ip to all interfaces for forwarding
    let id_to_attach = root_id.clone();
    mapper.register_handler(move |e| {
        let HardwareEvent::InterfacesUp(interfaces) = e else {
            return;
        };

        for interface in interfaces {
            platform::attach_node_id_ip(interface.name, &id_to_attach)
                .expect("attaching node-id ip should be possible");
        }
    });
    mapper.blocking_refresh();

    let fwd_table = NativeFwdTables::new(args.nftables_conf);

    let ip_cache = Arc::new(RwLock::new(HashMap::new()));
    let channel = kira_lib::messaging::udp::async_channel(
        args.socket_port,
        ip_cache,
        mapper,
        ProtocolMessageFormat::MessagePack,
        excluded_interfaces,
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
    tracing::info!(socket_address = %addr, "Bound to socket");

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
