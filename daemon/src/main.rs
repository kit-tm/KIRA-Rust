use std::collections::HashMap;
use std::sync::Arc;

use clap::Parser;
use tokio::sync::RwLock;

use r2kad_daemon_lib::{Node, NodeConfig};
use r2kad_lib::domain::NodeId;
use r2kad_lib::forwarding::in_memory_tables::InMemoryFwdTables;
use r2kad_lib::messaging::format::ProtocolMessageFormat;
use r2kad_lib::messaging::sync_wrapper::SyncWrapper;
use r2kad_lib::messaging::PNetInterfaceMapper;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    #[clap(short, long, value_parser, env = "SOCKET_PORT", default_value = "8080")]
    socket_port: u16,
    #[clap(short, long, value_parser, env = "NODE_ID")]
    root_id: Option<NodeId>,
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

    let mapper = PNetInterfaceMapper::new();
    mapper.blocking_refresh();

    let fwd_table = InMemoryFwdTables::new();

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

    let addr = message_sender
        .local_addr()
        .expect("failed to get bind addr");
    log::debug!("Using Address: {}", addr);

    let node = Node::new(
        NodeConfig::default(),
        root_id,
        Arc::clone(&runtime),
        vec![Box::new(message_receiver)],
        SyncWrapper::new(message_sender, Arc::clone(&runtime)),
        fwd_table,
    );

    let _handle = node.start();

    runtime
        .block_on(tokio::signal::ctrl_c())
        .expect("failed to register ctrl-c handler");
}
