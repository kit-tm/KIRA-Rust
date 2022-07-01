//! An Implementation of a Routing Daemon using a single Thread and running each usecase in a separate
//! async Task.
//!
//! This implementation uses the async [https://tokio.rs] runtime to support
//! running the daemon on a single threaded environment but also reading from
//! multiple ports at once.
//!
//! This is a naive implementation.
//! More advanced implementations may use load balancing or other advanced optimizations.

use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;

use tokio::sync::broadcast;
use tokio::sync::broadcast::Receiver;

use r2kad_lib::context::{UseCaseContext, TokioContext};
use r2kad_lib::domain::neighbor_hash_table::NeighborHashTable;
use r2kad_lib::domain::unlimited_neighbors_routing_table::UnlimitedNeighborsRoutingTable;
use r2kad_lib::domain::{FlatRoutingTable, NodeId, PNSStrategy, DEFAULT_BUCKET_SIZE};
use r2kad_lib::messaging::InMemoryMessageHub;
use r2kad_lib::node::Config;
use r2kad_lib::runtime::TokioRuntime;
use r2kad_lib::use_cases::bootstrap::BootstrapUseCase;
use r2kad_lib::use_cases::{UseCase, UseCaseEvent};

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

    // TODO: Read from CLI
    let config = Config::default();

    // Setup the Broadcaster which is necessary for the runtime to send messages to usecases
    let (broadcaster, _) = broadcast::channel::<UseCaseEvent>(100);

    // Create a Routing Table which stores ALL neighbors
    let routing_table = UnlimitedNeighborsRoutingTable::from(
        FlatRoutingTable::<DEFAULT_BUCKET_SIZE, 1>::new(root_id.clone())
            .expect("invalid flat Routing Table parameters"),
    );

    let insertion_strategy = PNSStrategy::<
        UnlimitedNeighborsRoutingTable<DEFAULT_BUCKET_SIZE, 1>,
        DEFAULT_BUCKET_SIZE,
    >::new();

    // Create the desired Context in which the Use Cases will run
    let context = Arc::new(TokioContext::new(
        root_id,
        routing_table,
        NeighborHashTable::new(),
        insertion_strategy,
        InMemoryMessageHub::new(),
        TokioRuntime::new(broadcaster.clone(), Arc::clone(&runtime)),
    ));

    let mut handles = Vec::new();

    let bootstrap_event_receiver = broadcaster.subscribe();
    let bootstrap_context = Arc::clone(&context);
    // Initialize the Use Cases
    let handle = runtime.spawn(async move {
        let use_case = BootstrapUseCase::new(config.bootstrap);

        UseCaseTask::new(
            "Bootstrap",
            bootstrap_context.deref(),
            bootstrap_event_receiver,
            use_case,
        )
        .start()
        .await;
    });
    handles.push(handle);

    // TODO:
    //  Start MessageReceivers and wait for message from broadcaster or MessageReceivers
    //  to delegate to use cases.

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
                break;
            }
        }
    }
}
