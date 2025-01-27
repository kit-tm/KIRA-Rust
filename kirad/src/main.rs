use std::collections::HashSet;
use std::ffi::OsString;
use std::num::NonZeroU32;

use clap::Parser;
use kira_forwarding::tables::in_memory_tables::InMemoryFwdTables;
use kira_lib::context::SyncContext;
use kira_lib::R2Kad;
use kirad_lib::format::ProtocolMessageFormat;
use kirad_lib::io::udp::async_channel;
use kirad_lib::underlay::observe_underlay;
use kirad_lib::Kira;

use kira_lib::domain::NodeId;
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
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");

    let args = Args::parse();

    let root_id: NodeId = args.root_id.unwrap_or_else(NodeId::random);
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
    let (connection, handle, underlay_updates) = runtime.block_on(async {
        observe_underlay(excluded_interfaces.clone())
            .expect("observing underlay neighborhood failed")
    });
    runtime.spawn(connection);

    //let fwd_table = NativeFwdTables::new(args.nftables_conf);
    let fwd_tables = InMemoryFwdTables::default();

    // create message sender and receiver
    let (pm_sender, pm_receiver) = runtime
        .block_on(async_channel(
            args.socket_port,
            ProtocolMessageFormat::MessagePack,
            handle,
            excluded_interfaces,
        ))
        .expect("socket creation failed");
    let addr = pm_sender.local_addr().expect("failed to get bind addr");
    tracing::info!(socket_address = %addr, "Bound to socket");

    let r2kad = R2Kad::<SyncContext<_, _, _, _>, 20>::builder()
        .root_id(args.root_id.unwrap_or_else(NodeId::random))
        .build();

    let kira = runtime.block_on(async {
        Kira::with_components(r2kad, fwd_tables, underlay_updates, pm_receiver, pm_sender)
    });

    runtime.block_on(kira.start());
}
