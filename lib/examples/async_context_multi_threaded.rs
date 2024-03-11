//! An Implementation of a Routing Daemon using a single Thread and running each usecase in a separate
//! async Task.
//!
//! This implementation uses the async [https://tokio.rs] runtime to support
//! running the daemon on a single threaded environment but also reading from
//! multiple ports at once.
//!
//! This is a naive implementation.
//! More advanced implementations may use load balancing or other advanced optimizations.

use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Display};
use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;

use tokio::sync::broadcast::Receiver;
use tokio::sync::{broadcast, RwLock};

use r2kad_lib::context::{ContextConfig, TokioContext, UseCaseContext};
use r2kad_lib::domain::physical_neighbor_table::PNTable;
use r2kad_lib::domain::unlimited_pn_routing_table::UnlimitedPNRoutingTable;
use r2kad_lib::domain::{
    FlatRoutingTable, InOrderCycleRemover, NodeId, PNSStrategy, ShortestFirstPathSimplifier,
    DEFAULT_BUCKET_SIZE,
};
use r2kad_lib::forwarding::in_memory_tables::InMemoryFwdTables;
use r2kad_lib::messaging::format::ProtocolMessageFormat;
use r2kad_lib::messaging::sync_wrapper::SyncWrapper;
use r2kad_lib::messaging::{AsyncProtocolMessageReceiver, PNetInterfaceMonitor};
use r2kad_lib::runtime::TokioRuntime;
use r2kad_lib::use_cases::vicinity_discovery::VicinityDiscovery;
use r2kad_lib::use_cases::{EventHandler, UseCase, UseCaseEvent, UseCaseState};

fn main() {
    // Setup the single threaded async runtime
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime"),
    );

    // Initialize the Logging Facade
    env_logger::init();

    let root_id: NodeId = std::env::var("NODE_ID")
        .map(|str_id| NodeId::from_str(&str_id).unwrap_or_else(|_| NodeId::random()))
        .unwrap_or_else(|_| NodeId::random());

    println!("Using NodeId {}", root_id);

    // Setup the Broadcaster which is necessary for the runtime to send messages to usecases
    let (broadcaster, _) = broadcast::channel::<UseCaseEvent>(100);

    // Create a Routing Table which stores ALL physical neighbors
    let routing_table = UnlimitedPNRoutingTable::from(
        FlatRoutingTable::<DEFAULT_BUCKET_SIZE, 1>::new(root_id.clone())
            .expect("invalid flat Routing Table parameters"),
    );

    let insertion_strategy = PNSStrategy::<
        UnlimitedPNRoutingTable<DEFAULT_BUCKET_SIZE, 1>,
        _,
        _,
        DEFAULT_BUCKET_SIZE,
    >::new(InOrderCycleRemover, ShortestFirstPathSimplifier);

    // Initialize IO Channel
    let ip_cache = Arc::new(RwLock::new(HashMap::new()));
    let channel = r2kad_lib::messaging::udp::async_channel(
        19219,
        ip_cache,
        PNetInterfaceMonitor::new(),
        ProtocolMessageFormat::MessagePack,
    );
    let (message_sender, mut message_receiver) = runtime
        .block_on(channel)
        .expect("failed to initialize IO channel");
    let receiver_broadcaster = broadcaster.clone();
    runtime.spawn(async move {
        loop {
            let (message, interface) = match message_receiver.recv().await {
                Ok(None) => continue,
                Ok(Some(value)) => value,
                Err(e) => {
                    log::error!("Error occurred while receiving message: {}", e);
                    break;
                }
            };

            if let Err(e) =
                receiver_broadcaster.send(UseCaseEvent::Message(message.clone(), interface.clone()))
            {
                log::error!("Failed to broadcast protocol message: {}", e);
            } else {
                log::debug!(
                    "Received ProtocolMessage from {} [interface: {}]",
                    message.source(),
                    interface
                );
            }
        }
        log::info!("Stopped receiver...");
    });

    let fwd_tables = InMemoryFwdTables::new();

    // Create the desired Context in which the Use Cases will run
    let context = Arc::new(TokioContext::new(ContextConfig {
        root_id: root_id.clone(),
        routing_table,
        pn_table: InMemoryPNTable::new(),
        insertion_strategy,
        message_sender: SyncWrapper::new(message_sender, Arc::clone(&runtime)),
        runtime: TokioRuntime::new(broadcaster.clone(), Arc::clone(&runtime)),
        forwarding_tables: fwd_tables,
        not_via: HashSet::default(),
    }));

    let mut handles = Vec::new();

    let handle_hello_event_receiver = broadcaster.subscribe();
    let handle_hello_context = Arc::clone(&context);
    // Initialize the Use Cases
    let handle = runtime.spawn(async move {
        let use_case = VicinityDiscovery::new(Default::default());

        UseCaseTask::new(
            "Vicinity Discovery",
            handle_hello_context.deref(),
            handle_hello_event_receiver,
            use_case,
        )
        .start()
        .await;
    });
    handles.push(handle);

    // IMPORTANT: The Runtime::block_on method drives progress in the CurrentThreadRuntime.
    //              Without that the tasks spawned in the runtime won't make any progress.
    let _ = runtime.block_on(futures::future::join_all(handles));
}

pub struct UseCaseTask<'a, UC, C> {
    name: String,
    use_case: UC,
    receiver: Receiver<UseCaseEvent>,
    context: &'a C,
}

impl<'a, UC, C> UseCaseTask<'a, UC, C> {
    fn new<S: Into<String>>(
        name: S,
        context: &'a C,
        receiver: Receiver<UseCaseEvent>,
        use_case: UC,
    ) -> Self {
        Self {
            name: name.into(),
            use_case,
            receiver,
            context,
        }
    }
}

impl<'a, UC, C> UseCaseTask<'a, UC, C>
where
    UC: UseCase<Context = C>,
    C: UseCaseContext,
    <UC as EventHandler>::Error: Display + Debug,
{
    async fn start(&mut self) {
        if let Err(e) = self.use_case.start(self.context) {
            log::error!("[{}] Failed to start: {}", self.name, e);
            return;
        }

        while let Ok(event) = self.receiver.recv().await {
            if let Err(e) = self.use_case.handle_event(self.context, event) {
                log::error!(
                    "[{}] Stopping due to error handling message: {:?}",
                    self.name,
                    e
                );
            }

            if self.use_case.state().is_error() {
                log::error!(
                    "UseCase reached unrecoverable error state. Aborting task '{}'",
                    self.name
                );
                break;
            }
        }
    }
}
