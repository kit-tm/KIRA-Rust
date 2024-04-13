use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs::{create_dir_all, File, OpenOptions};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use r2kad_lib::forwarding::native_tables::NativeFwdTables;
use r2kad_lib::forwarding::platform;
use r2kad_lib::hardware_events::{HardwareEvent, HardwareEventRegistry};
use tokio::sync::{mpsc, RwLock};

use r2kad_daemon_lib::{Node, NodeConfig};
use r2kad_lib::domain::NodeId;
use r2kad_lib::messaging::format::ProtocolMessageFormat;
use r2kad_lib::messaging::sync_wrapper::SyncWrapper;
use r2kad_lib::messaging::{AsyncProtocolMessageReceiver, PNetInterfaceMonitor};
use tokio::time::sleep;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(short, long, value_parser, env = "SOCKET_PORT", default_value = "19219")]
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
    // Initialize the Logging Facade
    env_logger::init();

    // Setup the single threaded async runtime
    let runtime = Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime"),
    );

    let args = Args::parse();

    let root_id: NodeId = args.root_id.unwrap_or_else(NodeId::random);
    println!("Using id {}", root_id);

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
    let channel = r2kad_lib::messaging::udp::async_channel(
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
