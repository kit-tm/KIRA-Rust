use std::collections::HashMap;
use std::sync::Arc;

use clap::Parser;
use tokio::sync::{broadcast, RwLock};

use r2kad_lib::context::TokioContext;
use r2kad_lib::domain::observable_routing_table::{ObservableRoutingTable, RoutingTableEvent};
use r2kad_lib::domain::unlimited_pn_routing_table::UnlimitedPNRoutingTable;
use r2kad_lib::domain::{
    bucket::DEFAULT_BUCKET_SIZE, FlatRoutingTable, InOrderCycleRemover, NodeId, PNSStrategy,
    PNTable, ShortestFirstPathSimplifier,
};
use r2kad_lib::messaging::format::ProtocolMessageFormat;
use r2kad_lib::messaging::sync_wrapper::SyncWrapper;
use r2kad_lib::messaging::{AsyncProtocolMessageReceiver, PNetPortMapper};
use r2kad_lib::runtime::TokioRuntime;
use r2kad_lib::use_cases::forward_protocol_message::ForwardPMUseCase;
use r2kad_lib::use_cases::handle_hello::HandleHelloUseCase;
use r2kad_lib::use_cases::overlay_neighborhood_discovery::ONDUseCase;
use r2kad_lib::use_cases::periodic_pn_advertising::PeriodicPNAdvertising;
use r2kad_lib::use_cases::random_probing::RandomProbingUseCase;
use r2kad_lib::use_cases::vicinity_discovery::VDUseCase;
use r2kad_lib::use_cases::{ContactEvent, UseCase, UseCaseEvent, UseCaseState};

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

    let args = Args::parse();

    // Setup the single threaded async runtime
    let runtime = Arc::new(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to build tokio runtime"),
    );

    let root_id: NodeId = args.root_id.unwrap_or_else(NodeId::random);

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
            RoutingTableEvent::UpdatedContact(contact) => ContactEvent::Updated(contact),
            _ => return,
        };
        if let Err(e) = observer_broadcaster.send(UseCaseEvent::Contact(contact_event)) {
            log::error!("Failed to broadcast ContactEvent: {}", e);
        }
    });
    routing_table.add_observer(|event| log::trace!("{}", event));

    // Initialize IO Channel
    let mapper = PNetPortMapper::new();
    mapper.blocking_refresh();

    let ip_cache = Arc::new(RwLock::new(HashMap::new()));
    let channel = r2kad_lib::messaging::udp::async_channel(
        args.socket_port,
        ip_cache,
        mapper,
        ProtocolMessageFormat::MessagePack,
    );
    let (message_sender, mut message_receiver) = runtime
        .block_on(channel)
        .expect("failed to initialize IO channel");

    let addr = message_sender
        .local_addr()
        .expect("failed to get bind addr");
    log::debug!("Using Address: {}", addr);

    let receiver_broadcaster = broadcaster.clone();
    let root_node_id = root_id.clone();
    runtime.spawn(async move {
        loop {
            let (message, port) = match message_receiver.recv().await {
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
                receiver_broadcaster.send(UseCaseEvent::Message(message.clone(), port.clone()))
            {
                log::error!("Failed to broadcast protocol message: {}", e);
            } else {
                log::debug!(
                    "Received ProtocolMessage from {} [{}]",
                    message.source(),
                    port
                );
            }
        }
        log::info!("Stopped receiver...");
    });

    // Create the desired Context in which the Use Cases will run
    let context = TokioContext::new(
        root_id,
        routing_table,
        PNTable::new(),
        PNSStrategy::<
            ObservableRoutingTable<
                UnlimitedPNRoutingTable<DEFAULT_BUCKET_SIZE, 1>,
                DEFAULT_BUCKET_SIZE,
            >,
            _,
            _,
            DEFAULT_BUCKET_SIZE,
        >::new(InOrderCycleRemover, ShortestFirstPathSimplifier),
        SyncWrapper::new(message_sender, Arc::clone(&runtime)),
        TokioRuntime::new(broadcaster, Arc::clone(&runtime)),
    );

    // Initialize the Use Cases

    let mut pn_advertising =
        PeriodicPNAdvertising::<_, DEFAULT_BUCKET_SIZE>::new(Default::default());
    if let Err(e) = pn_advertising.start(&context) {
        log::error!("Failed to start PN Probing UseCase: {}", e);
        return;
    }

    let mut random_probing = RandomProbingUseCase::new(Default::default());
    if let Err(e) = random_probing.start(&context) {
        log::error!("Failed to start Random Probing UseCase: {}", e);
        return;
    }

    let mut handle_hello = HandleHelloUseCase::new(Default::default());
    if let Err(e) = handle_hello.start(&context) {
        log::error!("Failed to start Hello Message UseCase: {}", e);
        return;
    }

    let mut on_disc = ONDUseCase::<_, DEFAULT_BUCKET_SIZE>::new(Default::default());
    if let Err(e) = on_disc.start(&context) {
        log::error!("Failed to start overlay neighbor discovery UseCase: {}", e);
        return;
    }

    let mut vicinity_disc = VDUseCase::new();
    if let Err(e) = vicinity_disc.start(&context) {
        log::error!("Failed to start overlay neighbor discovery UseCase: {}", e);
        return;
    }

    let mut forward_message = ForwardPMUseCase::new();
    if let Err(e) = forward_message.start(&context) {
        log::error!("Failed to start forward protocol messages UseCase: {}", e);
        return;
    }

    // Wait for MessageReceivers or runtime to emit events and delegate to Use Cases
    // IMPORTANT: The Runtime::block_on method drives progress in the CurrentThreadRuntime.
    //              Without that the tasks spawned in the runtime won't make any progress.
    while let Ok(event) = runtime.block_on(receiver.recv()) {
        log::trace!("Processing event {:?}", event);

        // Delegate Messages to UseCases
        if let Err(e) = forward_message.handle_event(&context, event.clone()) {
            log::error!(
                "Forwarding protocol message returned error handling message: {}",
                e
            );
        }
        if let Err(e) = pn_advertising.handle_event(&context, event.clone()) {
            log::error!(
                "Physical Neighbor Probing returned error handling message: {}",
                e
            );
        }
        if let Err(e) = random_probing.handle_event(&context, event.clone()) {
            log::error!("Random Probing returned error handling message: {}", e);
        }
        if let Err(e) = handle_hello.handle_event(&context, event.clone()) {
            log::error!("Handling Hello message returned error: {}", e);
        }
        if let Err(e) = on_disc.handle_event(&context, event.clone()) {
            log::error!(
                "Overlay Neighborhood Discovery returned error handling message: {}",
                e
            );
        }
        if let Err(e) = vicinity_disc.handle_event(&context, event) {
            log::error!("Vicinity Discovery returned error handling message: {}", e);
        }

        // Check States as returning an error doesn't show an unrecoverable error
        let states: Vec<&(dyn UseCaseState)> = vec![
            forward_message.state(),
            pn_advertising.state(),
            random_probing.state(),
            handle_hello.state(),
            on_disc.state(),
            vicinity_disc.state(),
        ];
        if states.iter().any(|use_case| use_case.is_error()) {
            log::error!("Some use case is in error state");
            break;
        }

        log::trace!("Processing event finished by all UseCases!");
    }
}
