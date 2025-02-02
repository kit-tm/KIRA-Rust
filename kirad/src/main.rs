use std::collections::HashSet;
use std::ffi::OsString;
use std::num::NonZeroU32;

use clap::Parser;
use futures::StreamExt;

use kira_lib::R2Kad;
use kirad_lib::Kira;

#[cfg(not(feature = "nft"))]
use kira_forwarding::tables::in_memory_tables::InMemoryFwdTables;
#[cfg(feature = "nft")]
use kira_forwarding::tables::native_tables::NativeFwdTables;

use kira_lib::context::SyncContext;
use kira_lib::domain::NodeId;
use kirad_lib::format::ProtocolMessageFormat;
use kirad_lib::io::udp::async_channel;
use kirad_lib::underlay::observe_underlay;

use signal_hook::consts::{SIGHUP, SIGINT, SIGKILL, SIGPIPE, SIGQUIT, SIGTERM};
use signal_hook_tokio::Signals;

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
    #[cfg_attr(feature = "tokio-console", clap(short, long))]
    tokio_console: bool,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    // Setup tracing environment
    {
        use tracing::Level;
        use tracing_subscriber::{
            filter::{filter_fn, EnvFilter, FilterExt, LevelFilter, Targets},
            layer::SubscriberExt,
            util::SubscriberInitExt,
            Layer,
        };

        #[cfg(feature = "tokio-console")]
        let console_layer = console_subscriber::spawn().with_filter(
            // enable required targets for console layer
            Targets::new()
                .with_target("tokio", Level::TRACE)
                .with_target("runtime", Level::TRACE)
                .with_default(LevelFilter::OFF),
        );
        // disable noisy netlink_proto debug messages
        // https://github.com/rust-netlink/netlink-proto/issues/19
        let netlink_proto_filter = filter_fn(|metadata| {
            !metadata.target().starts_with("netlink_proto") || metadata.level() != &Level::DEBUG
        });
        let fmt_layer = tracing_subscriber::fmt::layer()
            .compact()
            .with_filter(EnvFilter::from_default_env().and(netlink_proto_filter));

        let reg = tracing_subscriber::registry().with(fmt_layer);
        #[cfg(feature = "tokio-console")]
        if args.tokio_console {
            reg.with(console_layer).init()
        } else {
            reg.init()
        }
        #[cfg(not(feature = "tokio-console"))]
        reg.init()
    }

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
    let (connection, handle, underlay_updates, netlink_handle) =
        observe_underlay(excluded_interfaces.clone())
            .expect("observing underlay neighborhood failed");
    tokio::task::Builder::new()
        .name("Underlay Connection")
        .spawn(connection)
        .unwrap();

    #[cfg(not(feature = "nft"))]
    let fwd_tables = InMemoryFwdTables::default();
    #[cfg(feature = "nft")]
    let fwd_tables =
        NativeFwdTables::new(root_id, args.nftables_conf, netlink_handle, handle.clone()).await;

    // TODO: attach NodeId-IP to every interface

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

    let r2kad = R2Kad::<SyncContext<_, _, _, _>, 20>::builder()
        .root_id(root_id)
        .build();

    let kira = Kira::with_components(r2kad, fwd_tables, underlay_updates, pm_receiver, pm_sender);
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
