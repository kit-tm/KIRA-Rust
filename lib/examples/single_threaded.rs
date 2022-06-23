//! An Implementation of a Routing Daemon using a single Thread.
//!
//! This implementation uses the async [https://tokio.rs] runtime to support
//! running the daemon on a single threaded environment but also reading from
//! multiple ports at once.

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::Arc;

use tokio::sync::broadcast;

use r2kad_lib::context::TokioContext;
use r2kad_lib::domain::unlimited_neighbors_routing_table::UnlimitedNeighborsRoutingTable;
use r2kad_lib::domain::{FlatRoutingTable, NodeId, DEFAULT_BUCKET_SIZE, DEFAULT_ID_SIZE};
use r2kad_lib::messaging::InMemoryMessageHub;
use r2kad_lib::runtime::TokioRuntime;
use r2kad_lib::usecases::bootstrap::{BootstrapConfig, BootstrapState, BootstrapUseCase};
use r2kad_lib::usecases::UseCaseEvent;

const ID_SIZE: usize = DEFAULT_ID_SIZE;

/// CLI Configuration.
#[derive(Debug, Default)]
struct Config {
    bootstrap: BootstrapConfig,
}

fn main() {
    // Setup the single threaded async runtime
    let runtime = Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime"),
    );

    // Initialize the Logging Facade
    env_logger::init();

    let root_id: NodeId<ID_SIZE> = std::env::var("NODE_ID")
        .expect("failed to get environment var")
        .parse()
        .unwrap_or_else(|_| NodeId::random());

    println!("Using NodeId {}", root_id);

    // TODO: Read from CLI
    let config = Config::default();

    // Setup the Broadcaster which is necessary for the runtime to send messages to usecases
    let (broadcaster, mut receiver) = broadcast::channel::<UseCaseEvent<ID_SIZE>>(100);

    // Create a Routing Table which stores ALL neighbors
    let routing_table = UnlimitedNeighborsRoutingTable::from(
        FlatRoutingTable::<ID_SIZE, DEFAULT_BUCKET_SIZE, 1>::new(root_id.clone())
            .expect("invalid flat Routing Table parameters"),
    );

    // Create the desired Context in which the Use Cases will run
    let context = Arc::new(TokioContext::new(
        root_id.clone(),
        routing_table,
        HashMap::new(),
        HashMap::new(),
        InMemoryMessageHub::new(),
        TokioRuntime::new(broadcaster.clone(), Arc::clone(&runtime)),
    ));

    // Initialize the Use Cases
    // TODO: Add other Use Cases as soon as implemented
    let mut use_case = BootstrapUseCase::new();
    use_case
        .start(context.deref(), &config.bootstrap)
        .expect("failed to start bootstrap use case");

    // TODO:
    //  Start MessageReceivers and wait for message from broadcaster or MessageReceivers
    //  to delegate to use cases.

    // Wait for MessageReceivers or runtime to emit events and delegate to Use Cases
    while let Ok(event) = runtime.block_on(receiver.recv()) {
        if let Err(e) = use_case.handle_event(context.deref(), &config.bootstrap, event) {
            log::error!("Bootstrap failed: {}", e);
            break;
        }

        match use_case.state() {
            BootstrapState::Error => {
                log::error!("Bootstrap stopped in Error state!");
                break;
            }
            BootstrapState::Finished => {
                log::debug!("Bootstrap finished!");
                break;
            }
            _ => {}
        }
    }
}
