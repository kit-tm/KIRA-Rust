//! An Implementation of a Routing Daemon using a single Thread.
//!
//! This implementation uses the async [https://tokio.rs] runtime to support
//! running the daemon on a single threaded environment but also reading from
//! multiple ports at once.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::broadcast;

use r2kad_lib::context::TokioContext;
use r2kad_lib::domain::unlimited_neighbors_routing_table::UnlimitedNeighborsRoutingTable;
use r2kad_lib::domain::{FlatRoutingTable, NodeId, DEFAULT_BUCKET_SIZE};
use r2kad_lib::messaging::InMemoryMessageHub;
use r2kad_lib::node::{Config, Node};
use r2kad_lib::runtime::TokioRuntime;
use r2kad_lib::usecases::UseCaseEvent;

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

    let root_id: NodeId = std::env::var("NODE_ID")
        .expect("failed to get environment var")
        .parse()
        .unwrap_or_else(|_| NodeId::random());

    println!("Using NodeId {}", root_id);

    // TODO: Read from CLI
    let config = Config::default();

    // Setup the Broadcaster which is necessary for the runtime to send messages to usecases
    let (broadcaster, mut receiver) = broadcast::channel::<UseCaseEvent>(100);

    // Create a Routing Table which stores ALL neighbors
    let routing_table = UnlimitedNeighborsRoutingTable::from(
        FlatRoutingTable::<DEFAULT_BUCKET_SIZE, 1>::new(root_id.clone())
            .expect("invalid flat Routing Table parameters"),
    );

    // Create the desired Context in which the Use Cases will run
    let context = TokioContext::new(
        root_id,
        routing_table,
        HashMap::new(),
        InMemoryMessageHub::new(),
        TokioRuntime::new(broadcaster, Arc::clone(&runtime)),
    );

    // Initialize the Use Cases
    let mut node = Node::new(config, context);

    if let Err(e) = node.start() {
        log::error!("Error starting node: {}", e);
        return;
    }

    // TODO:
    //  Start MessageReceivers and wait for message from broadcaster or MessageReceivers
    //  to delegate to use cases.

    // Wait for MessageReceivers or runtime to emit events and delegate to Use Cases
    // IMPORTANT: The Runtime::block_on method drives progress in the CurrentThreadRuntime.
    //              Without that the tasks spawned in the runtime won't make any progress.
    while let Ok(event) = runtime.block_on(receiver.recv()) {
        if let Err(e) = node.handle_message(event) {
            log::error!("Error handling message: {}", e);
            break;
        }
    }
}
