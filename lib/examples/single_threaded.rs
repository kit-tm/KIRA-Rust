//! An Implementation of a Routing Daemon using a single Thread.
//!
//! This implementation uses the async [https://tokio.rs] runtime to support
//! running the daemon on a single threaded environment but also reading from
//! multiple ports at once.

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;

use tokio::sync::{broadcast, RwLock};

use r2kad_lib::context::{ContextConfig, TokioContext, UseCaseContext};
use r2kad_lib::domain::observable_routing_table::{ObservableRoutingTable, RoutingTableEvent};
use r2kad_lib::domain::unlimited_pn_routing_table::UnlimitedPNRoutingTable;
use r2kad_lib::domain::{FlatRoutingTable, InOrderCycleRemover, NodeId, PNSStrategy, ShortestFirstPathSimplifier, DEFAULT_BUCKET_SIZE, InMemoryPNTable};
use r2kad_lib::forwarding::in_memory_tables::InMemoryFwdTables;
use r2kad_lib::messaging::format::ProtocolMessageFormat;
use r2kad_lib::messaging::sync_wrapper::SyncWrapper;
use r2kad_lib::messaging::{AsyncProtocolMessageReceiver, PNetInterfaceMonitor};
use r2kad_lib::runtime::TokioRuntime;
use r2kad_lib::use_cases::forward_protocol_message::ForwardProtocolMessage;
use r2kad_lib::use_cases::handle_overlay_discovery::HandleOverlayDiscovery;
use r2kad_lib::use_cases::overlay_neighborhood_discovery::OverlayNeighborhoodDiscovery;
use r2kad_lib::use_cases::path_probing::PathProbing;
use r2kad_lib::use_cases::random_overlay_discovery::RandomOverlayDiscovery;
use r2kad_lib::use_cases::vicinity_discovery::VicinityDiscovery;
use r2kad_lib::use_cases::{
    ContactEvent, EventHandler, HandlingResult, UseCase, UseCaseEvent, UseCaseState,
};

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

    let root_id: NodeId = std::env::var("NODE_ID")
        .map(|str_id| NodeId::from_str(&str_id).unwrap_or_else(|_| NodeId::random()))
        .unwrap_or_else(|_| NodeId::random());

    println!("Using NodeId {}", root_id);

    // Setup the Broadcaster which is necessary for the runtime to send messages to usecases
    let (broadcaster, mut receiver) = broadcast::channel::<UseCaseEvent>(100);

    // Create a Routing Table which stores ALL physical neighbors
    let mut routing_table = ObservableRoutingTable::from(UnlimitedPNRoutingTable::from(
        FlatRoutingTable::<DEFAULT_BUCKET_SIZE, 1>::new(root_id.clone())
            .expect("invalid flat Routing Table parameters"),
    ));

    // Add observer which emits to broadcaster
    let observer_broadcaster = broadcaster.clone();
    routing_table.add_observer(move |event| {
        let contact_event = match event {
            RoutingTableEvent::NewContact(contact) => ContactEvent::New(contact),
            RoutingTableEvent::RemovedContact(contact) => ContactEvent::Removed(contact),
            RoutingTableEvent::UpdatedContact { old, new } => ContactEvent::Updated { old, new },
            _ => return,
        };
        if let Err(e) = observer_broadcaster.send(UseCaseEvent::Contact(contact_event)) {
            log::error!("Failed to broadcast ContactEvent: {}", e);
        }
    });
    routing_table.add_observer(|event| log::trace!("{}", event));

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
    let root_node_id = root_id.clone();
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

            if message.source() == &root_node_id {
                // ignoring messages from us
                continue;
            }

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
    let context = TokioContext::new(ContextConfig {
        root_id,
        routing_table,
        pn_table: InMemoryPNTable::new(),
        insertion_strategy: PNSStrategy::<
            ObservableRoutingTable<
                UnlimitedPNRoutingTable<DEFAULT_BUCKET_SIZE, 1>,
                DEFAULT_BUCKET_SIZE,
            >,
            _,
            _,
            DEFAULT_BUCKET_SIZE,
        >::new(InOrderCycleRemover, ShortestFirstPathSimplifier),
        message_sender: SyncWrapper::new(message_sender, Arc::clone(&runtime)),
        runtime: TokioRuntime::new(broadcaster, Arc::clone(&runtime)),
        forwarding_tables: fwd_tables,
        not_via: HashSet::default(),
    });

    // Initialize the Use Cases

    let mut forward_message = ForwardProtocolMessage::default();
    if let Err(e) = forward_message.start(&context) {
        log::error!("Failed to start forwarding UseCase: {}", e);
        return;
    }

    let random_probing = RandomOverlayDiscovery::new(Default::default());
    if let Err(e) = random_probing {
        log::error!("Invalid configuration: {}", e);
        return;
    }
    let mut random_probing = random_probing.unwrap();
    if let Err(e) = random_probing.start(&context) {
        log::error!("Failed to start Random Probing UseCase: {}", e);
        return;
    }

    let on_disc = OverlayNeighborhoodDiscovery::<_, DEFAULT_BUCKET_SIZE>::new(Default::default());
    if let Err(e) = on_disc {
        log::error!("Invalid configuration: {}", e);
        return;
    }
    let mut on_disc = on_disc.unwrap();
    if let Err(e) = on_disc.start(&context) {
        log::error!("Failed to start overlay neighbor discovery UseCase: {}", e);
        return;
    }

    let mut vicinity_disc = VicinityDiscovery::new(Default::default());
    if let Err(e) = vicinity_disc.start(&context) {
        log::error!("Failed to start overlay neighbor discovery UseCase: {}", e);
        return;
    }

    let mut path_probing = PathProbing::new(Default::default());
    if let Err(e) = path_probing.start(&context) {
        log::error!("Failed to start path probing UseCase: {}", e);
        return;
    }

    // Initialize common tasks

    let mut handle_overlay_discovery =
        HandleOverlayDiscovery::new(Default::default()).expect("default grouping should be valid");

    // Wait for MessageReceivers or runtime to emit events and delegate to Use Cases
    // IMPORTANT: The Runtime::block_on method drives progress in the CurrentThreadRuntime.
    //              Without that the tasks spawned in the runtime won't make any progress.
    while let Ok(event) = runtime.block_on(receiver.recv()) {
        log::trace!("Processing event {:?}", event);

        match forward_message.handle_event(&context, event.clone()) {
            Err(e) => log::error!(
                "Forwarding protocol message returned error handling message: {}",
                e
            ),
            Ok(HandlingResult::Handled) => continue, /* Skip delegation to use cases */
            Ok(HandlingResult::NotHandled) => { /* Delegate event to use cases  */ }
        }
        if let Err(e) = handle_overlay_discovery.handle_event(&context, event.clone()) {
            log::error!("Handling overlay discovery failed: {}", e);
        }

        if let Err(e) = random_probing.handle_event(&context, event.clone()) {
            log::error!("Random Probing returned error handling message: {}", e);
        }
        if let Err(e) = on_disc.handle_event(&context, event.clone()) {
            log::error!(
                "Overlay Neighborhood Discovery returned error handling message: {}",
                e
            );
        }
        if let Err(e) = vicinity_disc.handle_event(&context, event.clone()) {
            log::error!("Vicinity Discovery returned error handling message: {}", e);
        }
        if let Err(e) = path_probing.handle_event(&context, event) {
            log::error!("Path Probing returned error handling message: {}", e);
        }

        // Check States as returning an error doesn't show an unrecoverable error
        let states: Vec<&(dyn UseCaseState)> = vec![
            forward_message.state(),
            random_probing.state(),
            on_disc.state(),
            vicinity_disc.state(),
            path_probing.state(),
        ];
        if states.iter().any(|use_case| use_case.is_error()) {
            log::error!("Some use case is in error state");
            break;
        }

        log::trace!("Processing event finished by all UseCases!");
    }
}
