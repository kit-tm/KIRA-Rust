use std::collections::HashSet;
use std::error::Error;
use std::fmt::Debug;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

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
use r2kad_lib::use_cases::derive_fwd_table_entries::DeriveFwdTableEntries;
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

use crate::errors::InjectMessageError;

#[derive(Default, Debug)]
pub struct NodeConfig {
    pub message_injection_enabled: bool,
    pub heuristic_enabled: bool,
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
///     the [AsyncProtocolMessageReceivers].
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
    handle: Option<JoinHandle<()>>,
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
                    return Err(InjectMessageError::SendFailed(message));
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
        if self.handle.is_none() {
            return;
        }

        if let Err(e) = self.broadcaster.send_event(UseCaseEvent::Shutdown) {
            log::error!("Failed to send shutdown event: {}", e);
        }
    }

    pub fn wait_for_shutdown(&mut self) {
        if let Some(handle) = self.handle.take() {
            if let Err(e) = handle.join() {
                log::error!("Failed to join node thread: {:?}", e);
            }
        }
    }
}

impl Drop for NodeHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl<S, FT> Node<S, FT>
where
    S: ProtocolMessageSender + Send + 'static,
    FT: ForwardingTables + Send + 'static,
    <FT as NodeIdTable>::Error: Error,
    <FT as PathIdTable>::Error: Error,
{
    pub fn start(self) -> NodeHandle {
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

            Node::handle_loop(
                config,
                root_id,
                sender,
                fwd_table,
                cloned_broadcaster,
                broadcast_receiver,
                async_receivers,
                runtime,
                injection_sender,
            )
        });

        NodeHandle {
            handle: Some(handle),
            broadcaster,
            injection_result_receiver: injection_receiver,
        }
    }

    fn handle_loop(
        config: NodeConfig,
        root_id: NodeId,
        sender: S,
        fwd_table: FT,
        broadcaster: UnboundedSender<UseCaseEvent>,
        mut broadcast_receiver: UnboundedReceiver<UseCaseEvent>,
        mut async_receivers: Receiver<Box<dyn AsyncProtocolMessageReceiver + Send>>,
        runtime: Arc<tokio::runtime::Runtime>,
        injection_sender: Sender<InjectionResult>,
    ) {
        log::info!("Using NodeId {}", root_id);

        let (fan_in_sender, mut fan_in_receiver) = mpsc::channel(100);

        // Create a task to fan in own created events
        let broadcast_fan_in_sender = fan_in_sender.clone();
        runtime.spawn(async move {
            while let Some(event) = broadcast_receiver.recv().await {
                if let Err(e) = broadcast_fan_in_sender.send(event).await {
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
        let observer_broadcaster = fan_in_sender.clone();
        routing_table.add_observer(move |event| {
            let contact_event = match event {
                RoutingTableEvent::NewContact(contact) => ContactEvent::New(contact),
                RoutingTableEvent::RemovedContact(contact) => ContactEvent::Removed(contact),
                RoutingTableEvent::UpdatedContact { new, old } => {
                    ContactEvent::Updated { new, old }
                }
                _ => return,
            };
            if let Err(e) = observer_broadcaster.blocking_send(UseCaseEvent::Contact(contact_event))
            {
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
                    log::trace!("Listening to receiver {:?}", message_receiver);
                    let mut messages_cache = Vec::new();
                    let interfaces = loop {
                        match message_receiver.recv().await {
                            Ok(None) => continue,
                            Ok(Some(value)) => messages_cache.push(value),
                            Err(RecvError::InterfacesDown(interfaces)) => {
                                if let Err(e) = receiver_broadcaster
                                    .send(UseCaseEvent::Hardware(HardwareEvent::InterfacesDown(
                                        interfaces,
                                    )))
                                    .await
                                {
                                    log::error!(
                                        "Failed to send hardware event to use cases: {}",
                                        e
                                    );
                                }
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
                        // Receive a bulk of messages
                        while let Ok(Some(value)) = message_receiver.try_recv().await {
                            messages_cache.push(value);
                        }

                        for (message, interface) in messages_cache.drain(..) {
                            if message.source() == &root_node_id {
                                // ignoring messages from us
                                continue;
                            }

                            if let Err(e) = receiver_broadcaster
                                .send(UseCaseEvent::Message(message.clone(), interface.clone()))
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
                        }
                    };
                    if let Err(e) = receiver_broadcaster
                        .send(UseCaseEvent::Hardware(HardwareEvent::InterfacesDown(
                            interfaces.clone(),
                        )))
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
            runtime: TokioRuntime::new(broadcaster, Arc::clone(&runtime)),
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

        // Initialize the Use Cases

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

        let mut inject_messages = if config.message_injection_enabled {
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
        while let Some(event) = runtime.block_on(fan_in_receiver.recv()) {
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
            ];
            // As Injection can be disabled -> Need to append.
            if let Some(state) = inject_messages.as_ref().map(InjectMessages::state) {
                states.push(state);
            }
            if states.iter().any(|use_case| use_case.is_error()) {
                log::error!("Some use case is in error state");
                break;
            }

            log::trace!("Processing event finished by all UseCases!");

            if let UseCaseEvent::Shutdown = event {
                break;
            }
        }
        log::trace!("Shutting down.");
        log::logger().flush();
    }
}

mod errors {
    use std::error::Error;
    use std::fmt::{Debug, Display, Formatter};

    use r2kad_lib::messaging::ProtocolMessage;

    #[derive(Debug)]
    pub enum InjectMessageError {
        BroadcastFailed(Box<dyn Debug>),
        Closed,
        SendFailed(ProtocolMessage),
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
                Self::SendFailed(message) => {
                    write!(f, "Failed to send protocol message: {:#?}", message)
                }
                Self::Isolated => write!(f, "Failed to send protocol message; Node is isolated"),
                Self::Timeout => write!(f, "Request took to long"),
            }
        }
    }

    impl Error for InjectMessageError {}
}
