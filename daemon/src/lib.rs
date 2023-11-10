use std::collections::HashSet;
use std::error::Error;
use std::fmt::Debug;
use std::fs::File;
use std::io::BufWriter;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use signal_hook::consts::{SIGHUP, SIGINT, SIGKILL, SIGPIPE, SIGQUIT, SIGTERM};
use signal_hook_tokio::Signals;
use tokio::sync::mpsc;
use tokio::sync::mpsc::{Receiver, Sender, UnboundedReceiver, UnboundedSender};
use tokio::time::Instant;

use r2kad_lib::broadcaster::Broadcaster;
use r2kad_lib::context::{ContextConfig, SyncContext, UseCaseContext};
use r2kad_lib::domain::bucket::DEFAULT_BUCKET_SIZE;
use r2kad_lib::domain::observable_routing_table::{ObservableRoutingTable, RoutingTableEvent};
use r2kad_lib::domain::unlimited_pn_routing_table::UnlimitedPNRoutingTable;
use r2kad_lib::domain::{
    FlatRoutingTable, InOrderCycleRemover, NetworkInterface, NodeId, PNSStrategy, PNTable,
    ShortestFirstPathSimplifier,
};
use r2kad_lib::forwarding::{ForwardingTables, NodeIdTable, PathIdTable};
use r2kad_lib::hardware_events::HardwareEvent;
use r2kad_lib::messaging::{
    AsyncProtocolMessageReceiver, FindNodeReqData, Nonce, ProtocolMessage, ProtocolMessageSender,
    RecvError,
};
use r2kad_lib::runtime::TokioRuntime;
use r2kad_lib::use_cases;
use r2kad_lib::use_cases::api_message_handling::HandleApiMessages;
use r2kad_lib::use_cases::forward_kelly_message::ForwardKellyMessageHandler;
use r2kad_lib::use_cases::derive_fwd_table_entries::DeriveFwdTableEntries;
use r2kad_lib::use_cases::explicit_path_management::{EPMConfig, ExplicitPathManagement};
use r2kad_lib::use_cases::failure_handling::FailureHandling;
use r2kad_lib::use_cases::forward_protocol_message::ForwardProtocolMessage;
use r2kad_lib::use_cases::handle_contact_update::{HandleContactUpdate, HandleContactUpdateConfig};
use r2kad_lib::use_cases::handle_overlay_discovery::HandleOverlayDiscovery;
use r2kad_lib::use_cases::inject_messages::{
    InjectMessages, InjectMessagesConfig, InjectionResult,
};
use r2kad_lib::use_cases::overlay_neighborhood_discovery::OverlayNeighborhoodDiscovery;
use r2kad_lib::use_cases::path_probing::PathProbing;
use r2kad_lib::use_cases::precompute_paths_and_path_ids::PrecomputePathIds;
use r2kad_lib::use_cases::random_overlay_discovery::RandomOverlayDiscovery;
use r2kad_lib::use_cases::vicinity_discovery::{VicinityDiscovery, VicinityDiscoveryConfig};
use r2kad_lib::use_cases::{
    ContactEvent, EventHandler, HandlingResult, InjectionMessageData, UseCase, UseCaseEvent,
    UseCaseState,
};
use crate::api::ApiConfig;

use crate::benchmark_log::{BenchmarkEntry, BenchmarkLog};
use crate::errors::InjectMessageError;
use crate::kelly_connector::GrpcServerConfig;

mod benchmark_log;
mod api;
mod kelly_connector;

#[derive(Default, Debug)]
pub struct NodeConfig {
    pub heuristic_enabled: bool,
    pub benchmark_path: Option<BufWriter<File>>,
}

/// The main structure.
///
/// Wrapped inside a struct to allow integration tests to test the executables setup.
///
/// ## Channels
///
/// Uses multiple channels to communicate between different parts of the application.
///
/// - Broadcaster (UnboundedSender): None of these events should be lost as some are required for
///     valid operation of the application.
/// - Fan-in-MPSC Channel: All events received through [AsyncProtocolMessageReceiver] and Broadcaster
///     are delegated to the fan-in. This channel has a limited size to create backpressure for
///     the [AsyncProtocolMessageReceivers](r2kad_lib::messaging::receiver::AsyncProtocolMessageReceiver).
///
/// Therefore events emitted through the Broadcaster won't be lost, but [AsyncProtocolMessageReceiver]s
/// won't be pulled until there is space in the fan-in channel.
pub struct Node<S, FT> {
    root_id: NodeId,
    runtime: Arc<tokio::runtime::Runtime>,
    async_receivers: Receiver<Box<dyn AsyncProtocolMessageReceiver + Send>>,
    sender: S,
    fwd_table: FT,
    config: NodeConfig,
}

impl<S: Debug, FT> Node<S, FT> {
    pub fn new(
        config: NodeConfig,
        root_id: NodeId,
        runtime: Arc<tokio::runtime::Runtime>,
        async_receivers: Receiver<Box<dyn AsyncProtocolMessageReceiver + Send>>,
        sender: S,
        fwd_table: FT,
    ) -> Node<S, FT> {
        log::info!(
            "Created node {} with receivers {:#?} and sender {:#?}",
            root_id,
            async_receivers,
            sender
        );
        Self {
            root_id,
            runtime,
            async_receivers,
            sender,
            fwd_table,
            config,
        }
    }
}

pub struct NodeHandle {
    broadcaster: UnboundedSender<UseCaseEvent>,
    injection_result_receiver: Receiver<InjectionResult>,
}

impl NodeHandle {
    pub fn send_find_and_wait_for_response(
        &mut self,
        req_data: FindNodeReqData,
        timeout: Duration,
    ) -> Result<(ProtocolMessage, NetworkInterface), InjectMessageError> {
        let nonce = Nonce::random();

        self.broadcaster
            .send_event(UseCaseEvent::InjectMessage(
                nonce.clone(),
                InjectionMessageData::FindNode(req_data),
            ))
            .map_err(|e| InjectMessageError::BroadcastFailed(Box::new(e)))?;

        let timer = Instant::now();

        loop {
            let result = self.injection_result_receiver.try_recv();
            match result {
                Ok(InjectionResult::SendFailed(message)) => {
                    log::error!("Failed to send message: {:#?}", message);
                    return Err(InjectMessageError::SendFailed);
                }
                Ok(InjectionResult::Isolated) => return Err(InjectMessageError::Isolated),
                Ok(InjectionResult::Answered((message, interface))) => {
                    if Some(&nonce) == message.nonce() {
                        return Ok((message, interface));
                    }
                }
                Err(mpsc::error::TryRecvError::Disconnected) => break,
                _ => {}
            }

            let elapsed = timer.elapsed();
            if elapsed >= timeout {
                return Err(InjectMessageError::Timeout);
            }
        }

        Err(InjectMessageError::Closed)
    }

    pub fn shutdown(&self) {
        if self.broadcaster.is_closed() {
            return;
        }

        if let Err(e) = self.broadcaster.send_event(UseCaseEvent::Shutdown) {
            log::error!("Failed to send shutdown event: {}", e);
        }
    }
}

impl Drop for NodeHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct HandleLoopConfig<S, FT> {
    config: NodeConfig,
    root_id: NodeId,
    sender: S,
    fwd_table: FT,
    broadcaster: UnboundedSender<UseCaseEvent>,
    broadcast_receiver: UnboundedReceiver<UseCaseEvent>,
    async_receivers: Receiver<Box<dyn AsyncProtocolMessageReceiver + Send>>,
    runtime: Arc<tokio::runtime::Runtime>,
    injection_sender: Option<Sender<InjectionResult>>,
}

impl<S, FT> Node<S, FT>
where
    S: ProtocolMessageSender + Send + 'static,
    FT: ForwardingTables + Send + 'static,
    <FT as NodeIdTable>::Error: Error,
    <FT as PathIdTable>::Error: Error,
{
    /// Initializes the event loop and runs it.
    ///
    /// This means this method only returns after node has shut down.
    pub fn blocking_start(self) {
        let Node {
            root_id,
            runtime,
            async_receivers,
            sender,
            config,
            fwd_table,
        } = self;
        let (broadcaster, broadcast_receiver) = mpsc::unbounded_channel();

        let node_id_env_name = format!("{:?}_NODE_ID", std::thread::current().id());
        std::env::set_var(node_id_env_name, root_id.to_string());

        Node::handle_loop(HandleLoopConfig {
            config,
            root_id,
            sender,
            fwd_table,
            broadcaster,
            broadcast_receiver,
            async_receivers,
            runtime,
            injection_sender: None,
        })
    }

    pub fn start(self) -> (NodeHandle, std::thread::JoinHandle<()>) {
        let Node {
            root_id,
            runtime,
            async_receivers,
            sender,
            config,
            fwd_table,
        } = self;
        let (broadcaster, broadcast_receiver) = mpsc::unbounded_channel();
        let (injection_sender, injection_receiver) = mpsc::channel(1);

        let cloned_broadcaster = broadcaster.clone();
        let handle = std::thread::spawn(move || {
            let node_id_env_name = format!("{:?}_NODE_ID", std::thread::current().id());
            std::env::set_var(node_id_env_name, root_id.to_string());

            Node::handle_loop(HandleLoopConfig {
                config,
                root_id,
                sender,
                fwd_table,
                broadcaster: cloned_broadcaster,
                broadcast_receiver,
                async_receivers,
                runtime,
                injection_sender: Some(injection_sender),
            })
        });

        (
            NodeHandle {
                broadcaster,
                injection_result_receiver: injection_receiver,
            },
            handle,
        )
    }

    fn handle_loop(config: HandleLoopConfig<S, FT>) {
        let HandleLoopConfig {
            config,
            root_id,
            sender,
            fwd_table,
            broadcaster,
            mut broadcast_receiver,
            mut async_receivers,
            runtime,
            injection_sender,
        } = config;

        log::info!("Using NodeId {}", root_id);

        let mut benchmark_log = BenchmarkLog::new();
        let mut bench_file_writer = config.benchmark_path.map(BufWriter::new);

        let (fan_in_sender, mut fan_in_receiver) =
            mpsc::channel::<(UseCaseEvent, Option<Instant>)>(100);

        let new_sender = fan_in_sender.clone();

        // Create a task to fan in own created events
        let broadcast_fan_in_sender = fan_in_sender.clone();
        runtime.spawn(async move {
            while let Some(event) = broadcast_receiver.recv().await {
                if let Err(e) = broadcast_fan_in_sender.send((event, None)).await {
                    log::error!("Failed to fan in broadcastet event: {}", e);
                    break;
                }
            }
            log::trace!("Closing broadcast fan in task...");
        });

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
                RoutingTableEvent::UpdatedContact { new, old } => {
                    ContactEvent::Updated { new, old }
                }
                _ => return,
            };
            if let Err(e) = observer_broadcaster.send(UseCaseEvent::Contact(contact_event)) {
                log::error!("Failed to broadcast ContactEvent: {}", e);
            }
        });
        routing_table.add_observer(|event| log::trace!("{}", event));

        // Initialize A Task for every receiver
        let rt = runtime.clone();
        let root_node_id = root_id.clone();
        runtime.spawn(async move {
            while let Some(mut message_receiver) = async_receivers.recv().await {
                let receiver_broadcaster = fan_in_sender.clone();
                let root_node_id = root_node_id.clone();
                rt.spawn(async move {
                    let interfaces = loop {
                        let (message, interface, started) = match message_receiver.recv().await {
                            Ok(None) => continue,
                            Ok(Some((message, interface))) => (message, interface, Instant::now()),
                            Err(RecvError::InterfacesDown(interfaces)) => {
                                if let Err(e) = receiver_broadcaster
                                    .send((
                                        UseCaseEvent::Hardware(HardwareEvent::InterfacesDown(
                                            interfaces,
                                        )),
                                        None,
                                    ))
                                    .await
                                {
                                    log::error!(
                                        "Failed to send hardware event to use cases: {}",
                                        e
                                    );
                                }
                                continue;
                            }
                            Err(RecvError::Closed(interfaces)) => {
                                break interfaces;
                            }
                            Err(e) => {
                                log::error!(
                                    "Error occurred while receiving message [retrying]: {}",
                                    e
                                );
                                continue;
                            }
                        };

                        if message.source() == &root_node_id {
                            // ignoring messages from us
                            continue;
                        }

                        if let Err(e) = receiver_broadcaster
                            .send((
                                UseCaseEvent::Message(message.clone(), interface.clone()),
                                Some(started),
                            ))
                            .await
                        {
                            log::error!("Failed to broadcast protocol message: {}", e);
                        } else {
                            log::trace!(
                                "Received ProtocolMessage from {} [{}]",
                                message.source(),
                                interface
                            );
                        }
                    };
                    if let Err(e) = receiver_broadcaster
                        .send((
                            UseCaseEvent::Hardware(HardwareEvent::InterfacesDown(
                                interfaces.clone(),
                            )),
                            None,
                        ))
                        .await
                    {
                        log::error!("Failed to send hardware event to use cases: {}", e);
                    }
                    log::debug!("Stopped receiver for interfaces {:?}", interfaces);
                });
            }
            log::trace!("Stopped listening to new protocol message receivers...");
        });

        // Create the desired Context in which the Use Cases will run
        let context_config = ContextConfig {
            root_id: root_id.clone(),
            routing_table,
            message_sender: sender,
            runtime: TokioRuntime::new(broadcaster.clone(), Arc::clone(&runtime)),
            insertion_strategy: PNSStrategy::<
                ObservableRoutingTable<
                    UnlimitedPNRoutingTable<DEFAULT_BUCKET_SIZE, 1>,
                    DEFAULT_BUCKET_SIZE,
                >,
                _,
                _,
                DEFAULT_BUCKET_SIZE,
            >::new(InOrderCycleRemover, ShortestFirstPathSimplifier),
            pn_table: PNTable::new(),
            forwarding_tables: fwd_table,
            not_via: HashSet::default(),
        };
        let context = SyncContext::new(context_config);

        // Start Signal handler to listen to OS signals
        runtime.spawn(async move {
            let mut signals: Signals = Signals::new([SIGHUP, SIGTERM, SIGINT, SIGQUIT, SIGPIPE])
                .expect("failed to create signals");

            let received = signals.next().await;

            match received {
                Some(SIGHUP) => println!("Received SIGHUP"),
                Some(SIGTERM) => println!("Received SIGTERM"),
                Some(SIGINT) => println!("Received SIGQUIT"),
                Some(SIGPIPE) => println!("Received SIGPIPE"),
                Some(SIGKILL) => println!("Received SIGKILL"),
                Some(signal) => println!("Received unsupported signal: {}", signal),
                None => println!("Closed before signal could be received"),
            }

            if let Err(e) = broadcaster.send(UseCaseEvent::Shutdown) {
                log::error!("Failed to broadcast signal triggered shutdown: {}", e);
            }
        });

        let api_config = ApiConfig::new(
            "0.0.0.0:3000".parse().unwrap(),
            root_id.clone().into(),
            new_sender
        );

        runtime.spawn(api::start_http_server(api_config));

        let grpc_config = GrpcServerConfig {
            address: "0.0.0.0:3001".to_string()
        };

        runtime.spawn(kelly_connector::start_grpc_server(grpc_config));

        // Initialize the Use Cases

        let mut api_handling = HandleApiMessages::default();

        let mut forward_kelly_message_handler = ForwardKellyMessageHandler::default();

        let mut forward_message = ForwardProtocolMessage::default();
        if let Err(e) = forward_message.start(&context) {
            log::error!("Failed to start forwarding UseCase: {}", e);
            return;
        }

        let mut random_probing = RandomOverlayDiscovery::new(Default::default())
            .expect("default grouping should be valid");
        if let Err(e) = random_probing.start(&context) {
            log::error!("Failed to start Random Probing UseCase: {}", e);
            return;
        }

        let mut on_disc =
            OverlayNeighborhoodDiscovery::<_, DEFAULT_BUCKET_SIZE>::new(Default::default())
                .expect("default grouping should be valid");
        if let Err(e) = on_disc.start(&context) {
            log::error!("Failed to start overlay neighbor discovery UseCase: {}", e);
            return;
        }

        let vicinity_config = VicinityDiscoveryConfig {
            heuristic_enabled: config.heuristic_enabled,
            ..Default::default()
        };
        let mut vicinity_disc = VicinityDiscovery::new(vicinity_config);
        if let Err(e) = vicinity_disc.start(&context) {
            log::error!("Failed to start overlay neighbor discovery UseCase: {}", e);
            return;
        }

        let mut path_probing = PathProbing::new(Default::default());
        if let Err(e) = path_probing.start(&context) {
            log::error!("Failed to start path probing UseCase: {}", e);
            return;
        }

        let mut derive_forwarding_tables = DeriveFwdTableEntries::new(Default::default());
        if let Err(e) = derive_forwarding_tables.start(&context) {
            log::error!("Failed to start path probing UseCase: {}", e);
            return;
        }

        let mut failure_handling = FailureHandling::new(Default::default());
        if let Err(e) = failure_handling.start(&context) {
            log::error!("Failed to start failure handling UseCase: {}", e);
            return;
        }

        let mut precomputation = PrecomputePathIds::new(root_id, Default::default());
        if precomputation.start(&context).is_err() {
            log::error!("Failed to start precomputation");
            return;
        }

        let mut explicit_path_management = ExplicitPathManagement::new(EPMConfig::default());
        if let Err(e) = explicit_path_management.start(&context) {
            log::error!("Failed to start explicit path management: {}", e);
        }

        let mut inject_messages = if let Some(injection_sender) = injection_sender {
            let mut inject_messages =
                InjectMessages::new(InjectMessagesConfig::default(), injection_sender)
                    .expect("default grouping should be valid");
            if let Err(e) = inject_messages.start(&context) {
                log::error!("Failed to start inject messages UseCase: {}", e);
                return;
            }
            Some(inject_messages)
        } else {
            None
        };

        // Initialize common tasks

        let mut handle_overlay_discovery = HandleOverlayDiscovery::new(Default::default())
            .expect("default grouping should be valid");

        let mut handle_update_contact =
            HandleContactUpdate::new(HandleContactUpdateConfig::default());

        // Wait for MessageReceivers or runtime to emit events and delegate to Use Cases
        // IMPORTANT: The Runtime::block_on method drives progress in the CurrentThreadRuntime.
        //              Without that the tasks spawned in the runtime won't make any progress.
        while let Some((event, start_time)) = runtime.block_on(fan_in_receiver.recv()) {
            log::trace!("Processing event {:?}", event);

            // Some precomputation to perform actions and delegate which are common tasks
            match forward_message.handle_event(&context, event.clone()) {
                Err(e) => log::error!(
                    "Forwarding protocol message returned error handling message: {}",
                    e
                ),
                Ok(HandlingResult::Handled) => continue, /* Skip delegation to other use cases */
                Ok(HandlingResult::NotHandled) => { /* Delegate event to use cases  */ }
            }
            if let Err(e) = handle_overlay_discovery.handle_event(&context, event.clone()) {
                log::error!("Handling overlay discovery failed: {}", e);
            }
            if let Err(e) = handle_update_contact.handle_event(&context, event.clone()) {
                log::error!("Handling contact update failed: {}", e);
            }

            // Actual use cases
            if let Err(e) = api_handling.handle_event(&context, event.clone()) {
                log::error!("Failure handling api event: {}", e)
            }
            if let Err(e) = forward_kelly_message_handler.handle_event(&context, event.clone()) {
                log::error!("Failure handling kelly message forwarding: {:?}", e)
            }

            if let Err(e) = failure_handling.handle_event(&context, event.clone()) {
                log::error!("Failure handling returned error handling message: {}", e);
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
            if let Err(e) = path_probing.handle_event(&context, event.clone()) {
                log::error!("Path Probing returned error handling message: {}", e);
            }
            if let Err(e) = derive_forwarding_tables.handle_event(&context, event.clone()) {
                log::error!("Path Probing returned error handling message: {}", e);
            }
            if precomputation
                .handle_event(&context, event.clone())
                .is_err()
            {
                log::error!("Precomputation returned error handling message");
            }
            if let Err(e) = explicit_path_management.handle_event(&context, event.clone()) {
                log::error!(
                    "Explicit path management returned error handling message: {}",
                    e
                );
            }
            if let Some(Err(e)) = inject_messages
                .as_mut()
                .map(|use_case| use_case.handle_event(&context, event.clone()))
            {
                log::error!("Injecting Messages returned error handling message: {}", e);
            }

            // Check States as returning an error doesn't show an unrecoverable error
            let mut states: Vec<&(dyn UseCaseState)> = vec![
                forward_message.state(),
                failure_handling.state(),
                random_probing.state(),
                on_disc.state(),
                vicinity_disc.state(),
                derive_forwarding_tables.state(),
                path_probing.state(),
                precomputation.state(),
                explicit_path_management.state(),
            ];
            // As Injection can be disabled -> Need to append.
            if let Some(state) = inject_messages.as_ref().map(InjectMessages::state) {
                states.push(state);
            }
            if states.iter().any(|use_case| use_case.is_error()) {
                log::error!("Some use case is in error state");
                break;
            }

            if let Some(start_time) = start_time {
                let took_time = start_time.elapsed();

                log::trace!(
                    "Took {} to process {:?} by all use cases!",
                    humantime::format_duration(took_time),
                    event
                );
                #[cfg(feature = "bench")]
                if let (UseCaseEvent::Message(message, _), Some(writer)) =
                    (&event, bench_file_writer.as_mut())
                {
                    let bench_entry = BenchmarkEntry::new(message, took_time);
                    benchmark_log.append_bench(bench_entry, writer);
                }
            }

            if let UseCaseEvent::Shutdown = event {
                break;
            }
        }
        log::debug!("Shutting down");
        log::logger().flush();
        #[cfg(feature = "bench")]
        if let Some(writer) = bench_file_writer.as_mut() {
            benchmark_log.flush(writer);
            log::trace!("Flushed benchmarks");
        }
    }
}

mod errors {
    use std::error::Error;
    use std::fmt::{Debug, Display, Formatter};

    #[derive(Debug)]
    pub enum InjectMessageError {
        BroadcastFailed(Box<dyn Debug>),
        Closed,
        SendFailed,
        Isolated,
        Timeout,
    }

    impl Display for InjectMessageError {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::BroadcastFailed(inner) => {
                    write!(f, "Failed to broadcast request injection: {:?}", inner)
                }
                Self::Closed => write!(f, "Channel to node closed while waiting for message"),
                Self::SendFailed => {
                    write!(f, "Failed to send protocol message")
                }
                Self::Isolated => write!(f, "Failed to send protocol message; Node is isolated"),
                Self::Timeout => write!(f, "Request took to long"),
            }
        }
    }

    impl Error for InjectMessageError {}
}
