use std::collections::HashSet;
use std::error::Error;
use std::fmt::Debug;
use std::fs::File;
use std::io::BufWriter;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use futures::{StreamExt, TryStreamExt};
use kira_lib::domain::protocol_event::DebugEvent;
use kira_lib::messaging::ProtocolMessage;
use rtnetlink::{Error as NlError, new_connection};
use signal_hook::consts::{SIGHUP, SIGINT, SIGKILL, SIGPIPE, SIGQUIT, SIGTERM};
use signal_hook_tokio::Signals;
use std::net::Ipv6Addr;
use tokio::sync::mpsc;
use tokio::sync::mpsc::{Receiver, Sender, UnboundedReceiver, UnboundedSender};
use tokio::time::Instant;
use tracing::{Level, span};

use kira_lib::context::{ContextConfig, SyncContext, UseCaseContext};
use kira_lib::domain::observable_routing_table::{ObservableRoutingTable, RoutingTableEvent};
use kira_lib::domain::unlimited_pn_routing_table::UnlimitedPNRoutingTable;
use kira_lib::domain::{
    FlatRoutingTable, InOrderCycleRemover, NodeId, PNSStrategy, ShortestFirstPathSimplifier,
    UnderlayNeighborDestination, UnderlayNeighborId, UnderlayNeighborUpdate,
};
use kira_lib::{Input, Output, R2Kad};

use kira_lib::use_cases::{ApiEvent, ContactEvent, InjectionMessageData};

use crate::benchmark_log::BenchmarkLog;
use crate::errors::InjectMessageError;
use crate::io::receiver::{AsyncProtocolMessageReceiver, ProtocolMessageReceiver};
use crate::io::sender::{AsyncProtocolMessageSender, ProtocolMessageSender};

mod benchmark_log;

#[cfg(feature = "api")]
pub mod api;
pub mod format;
pub mod io;
pub mod r2kad;
pub mod underlay;

#[derive(Default, Debug)]
pub struct NodeConfig {
    pub heuristic_enabled: bool,
    pub benchmark_path: Option<BufWriter<File>>,
}

#[cfg(feature = "small_buckets")]
const BUCKET_SIZE: usize = 3;
#[cfg(not(feature = "small_buckets"))]
use kira_lib::domain::bucket::DEFAULT_BUCKET_SIZE;
#[cfg(not(feature = "small_buckets"))]
const BUCKET_SIZE: usize = DEFAULT_BUCKET_SIZE;

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
///     the [AsyncProtocolMessageReceivers](kira_lib::messaging::receiver::AsyncProtocolMessageReceiver).
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
        log::trace!(
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
                Some(&nonce).cloned(),
                InjectionMessageData::FindNode(req_data),
            ))
            .map_err(|e| InjectMessageError::BroadcastFailed(Box::new(e)))?;

        let timer = Instant::now();

        loop {
            let result = self.injection_result_receiver.try_recv();
            match result {
                Ok(InjectionResult::SendFailed(nonce)) => {
                    log::error!("Failed to send message with nonce {:?}", nonce);
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
        log::info!("Using bucket size k={}", BUCKET_SIZE);

        let mut _benchmark_log = BenchmarkLog::new();
        let mut _bench_file_writer = config.benchmark_path.map(BufWriter::new);

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
            FlatRoutingTable::<BUCKET_SIZE, 1>::new(root_id.clone())
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
                RoutingTableEvent::UpdatedBucket(bucket) => ContactEvent::BucketUpdated(bucket),
                RoutingTableEvent::NewBucket(bucket) => ContactEvent::NewBucket(bucket),
                _ => return,
            };
            if let Err(e) = observer_broadcaster.send(UseCaseEvent::Contact(contact_event)) {
                log::error!("Failed to broadcast ContactEvent: {}", e);
            }
        });
        routing_table.add_observer(|event| log::trace!(target: "routing_table", "{}", event));

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
                            log::debug!("Ignoring message from us [{:?}]", message);
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
                ObservableRoutingTable<UnlimitedPNRoutingTable<BUCKET_SIZE, 1>, BUCKET_SIZE>,
                _,
                _,
                BUCKET_SIZE,
            >::new(InOrderCycleRemover, ShortestFirstPathSimplifier),
            pn_table: NativePNTable::new(),
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

        #[cfg(feature = "api")]
        let api_config = api::ApiConfig::new(
            "[::]:8080".parse().unwrap(),
            root_id.clone().into(),
            new_sender,
        );

        #[cfg(feature = "api")]
        runtime.spawn(api::start_http_server(api_config));

        // remove IP addresses from interfaces
        runtime
            .block_on(async {
                let (connection, handle, _) = new_connection().unwrap();
                runtime.spawn(connection);
                let mut addresses = handle
                    .address()
                    .get()
                    .set_address_filter(Ipv6Addr::from(&root_id).into())
                    .execute();
                while let Some(addr) = addresses.try_next().await? {
                    handle.address().del(addr).execute().await?;
                }
                Ok::<(), NlError>(())
            })
            .expect("removing Ips should work");

        log::debug!("Shutting down");
        log::logger().flush();
    }
}

mod errors {
    use derive_more::{Display, Error};
    use std::fmt::Debug;

    #[derive(Debug, Display, Error)]
    pub enum InjectMessageError {
        #[display("Failed to broadcast request injection: {_0:?}")]
        BroadcastFailed(#[error(not(source))] Box<dyn Debug>),
        #[display("Channel to node closed while waiting for message")]
        Closed,
        #[display("Failed to send protocol message")]
        SendFailed,
        #[display("Failed to send protocol message; Node is isolated")]
        Isolated,
        #[display("Request took to long")]
        Timeout,
    }
}
