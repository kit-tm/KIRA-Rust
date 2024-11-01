//! Source code documentation for the library crate of the KIRA implementation created at the
//! [institute for telematics](https://telematics.tm.kit.edu/index.php) at the
//! [Karlsruher Institute of Technology (KIT)](https://www.kit.edu).
//!
//! # Logging Targets
//!
//! Logging is implemented through the [log](https://crates.io/crates/log) crate.
//! While sometimes default logging targets based on module structure are used, some special
//! logging targets have been added.
//!
//! - `routing_table`: Updates to the routing table.
//! - `pn_table`: Updated to the physical neighbor table.
//! - `message_sender`: Information about sending protocol messages.
//! - `message_receiver`: Information about receiving protocol messages.
//! - `network_interfaces`: Logging of updates to the network interfaces.
//! - `in_memory_fwd_table`: Updates to the stub implementation of the forwarding tables [InMemoryFwdTable](forwarding::in_memory_tables::InMemoryFwdTables)
//!
//! Use Case related:
//!
//! - `derive_fwd_table_entries`: Logs of the use case [DeriveFwdTableEntries](use_cases::derive_fwd_table_entries::DeriveFwdTableEntries).
//! - `explicit_path_management`: Use case [ExplicitPathManagement](use_cases::explicit_path_management::ExplicitPathManagement)
//! - `failure_handling`: Use case [FailureHandling](use_cases::failure_handling::FailureHandling)
//! - `forward_protocol_message`: Use case [ForwardProtocolMessage](use_cases::forward_protocol_message::ForwardProtocolMessage)
//! - `handle_contact_update`: Use case [HandleContactUpdate](use_cases::handle_contact_update::HandleContactUpdate)
//! - `handle_overlay_discovery`: Use case [HandleOverlayDiscovery](use_cases::handle_overlay_discovery::HandleOverlayDiscovery)
//! - `inject_messages`: Use case [InjectMessages](use_cases::inject_messages::InjectMessages)
//! - `overlay_neighborhood_discovery`: Use case [OverlayNeighborhoodDiscovery](use_cases::overlay_neighborhood_discovery::OverlayNeighborhoodDiscovery)
//! - `path_probing`: Use case [PathProbing](use_cases::path_probing::PathProbing)
//! - `precompute_paths_and_path_ids`: Use case [PathProbing](use_cases::precompute_paths_and_path_ids::PrecomputePathIds)
//! - `random_overlay_discovery`: Use case [RandomOverlayDiscovery](use_cases::random_overlay_discovery::RandomOverlayDiscovery)
//! - `vicinity_discovery`: Use case [VicinityDiscovery](use_cases::vicinity_discovery::VicinityDiscovery)
//! - `distributed_hash_table`: Use case [DistributedHashTable](use_cases::distributed_hash_table::DistributedHashTable)
//! - `distributed_hash_table_injector`: Use case [DistributedHashTableInjector](use_cases::distributed_hash_table_injector::DistributedHashTableInjector)
//!
//! # Authors
//!
//! - Moritz Hepp (former student at KIT)

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod broadcaster;
pub mod context;
pub mod domain;
pub mod forwarding;
pub mod hardware_events;
pub mod messaging;
#[cfg(feature = "pnet")]
pub mod pnet_interface_monitor;
pub mod runtime;
pub mod use_cases;
pub mod utils;

use std::{collections::VecDeque, time::Instant};
use thiserror::Error;

#[doc(inline)]
pub use crate::domain::protocol_event::{Input, Output};

use crate::{
    context::UseCaseContext,
    domain::NodeId,
    runtime::UseCaseRuntime,
    use_cases::{
        derive_fwd_table_entries::DeriveFwdTableEntries,
        distributed_hash_table::{DefaultExpiringHashTable, DistributedHashTable},
        distributed_hash_table_injector::DistributedHashTableInjector,
        explicit_path_management::ExplicitPathManagement,
        failure_handling::FailureHandling,
        forward_protocol_message::ForwardProtocolMessage,
        inject_messages::InjectMessages,
        overlay_neighborhood_discovery::OverlayNeighborhoodDiscovery,
        path_probing::PathProbing,
        precompute_paths_and_path_ids::PrecomputePathIds,
        random_overlay_discovery::RandomOverlayDiscovery,
        vicinity_discovery::{VicinityDiscovery, VicinityDiscoveryConfig},
        UseCase, UseCaseEvent,
    },
    utils::sync::Sender,
};

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum R2KadError {}

pub type Result<T> = core::result::Result<T, R2KadError>;

/// Protocol instance of R²/KAD. This is the main handle of the library.
///
/// # Usage
///
/// ```
/// use std::time::Instant;
///
/// use kira_lib::{R2Kad, Input, Output}
///
///     let mut r2kad = R2Kad::new();
///
/// loop {
///     let timeout = match r2kad.poll_output().unwrap() {
///         Output::Timeout(v) => v,
///         Output::SendProtocolMessage(message, destination) => {
///             // TODO: Send data to remote peer.
///             continue; // poll again
///         }
///         Output::UpdateForwardingTables(update_req) => {
///             // TODO: Update the forwarding tables.
///             continue; // poll again
///         }
///     };
///
///     // Wait for two types of events:
///     //   1. Network input or Debug requests
///     //   2. Timeout
///     match tokio::time::timeout(Instant::now().duration_since(timeout), async move {
///         // TODO: Receive data from remote peers.
///         todo!("receive protocol messages")
///     })
///     .await
///     {
///         Ok(input) => r2kad.receive_event(input).unwrap(),
///         Err(_) => continue, // poll again
///     }
/// }
/// ```
pub struct R2Kad<C, const BUCKET_SIZE: usize> {
    root: NodeId,
    rx_events: VecDeque<Input>,
    context: C,
    pipeline: R2KadPipeline<C, BUCKET_SIZE>,
}

impl<C, const BUCKET_SIZE: usize> R2Kad<C, BUCKET_SIZE> {
    pub fn new(context: C) -> Result<Self> {
        let root = NodeId::random();
        Self::with_root(root, context)
    }

    pub fn with_root(root: NodeId, context: C) -> Result<Self> {
        // TODO: context builder
        Ok(Self {
            root,
            rx_events: Vec::default(),
            context,
            pipeline: R2KadPipeline::default(),
        })
    }

    /// Receive an [Input] event.
    ///
    /// This function by itself will not drive *any* progress on the protocol.
    /// To process [Input] events it is necessary to call [process_received](Self::process_received).
    ///
    /// The time an [Input] was received is also irrelevant to the protocol.
    /// Only the time the [Input] is being processed is relevant.
    pub fn receive_event(&mut self, event: Input) {
        self.rx_events.push_back(event);
    }
}

impl<C, S, const BUCKET_SIZE: usize> R2Kad<C, BUCKET_SIZE>
where
    S: Sender<Output>,
    C: UseCaseContext<Runtime = UseCaseRuntime<S>>,
{
    /// Process received [Input] event.
    ///
    /// The call returns after the protocol decides it's a good time to stop
    /// processing.
    ///
    /// # Return
    ///
    /// Hint on when to be called again.
    ///
    /// There are three options:
    ///
    /// 1. `None`: It's at users discretion on when to call again.
    /// 2. `Instant::now`: The protocol stopped processing received [Input]
    ///     events even though there are some buffered left.
    /// 3.  future `Instant`: There is no left [Input] data available but
    ///     the protocol is waiting on some timer.
    ///
    /// # Usage
    ///
    /// In all three cases the method can be called immediately after new
    /// data is received with [receive_event](Self::receive_event).
    ///
    /// The method can be called after the returned [Instant] is in the past.
    pub fn process_received(
        &mut self,
        now: Instant,
        sender_callback: S,
    ) -> Result<Option<Instant>> {
        let mut runtime = self.context.runtime_mut();

        runtime.update_sender(sender_callback);

        // only take one use case event
        if let Some(event) = self.rx_events.pop_front() {
            runtime.spawn_event(event.into());
        }

        // consume all events generated inside the runtime
        loop {
            let Some(event) = runtime.next_event(now) else {
                break;
            };

            self.pipeline.process_event(&self.context, event)?;
        }

        if !self.rx_events.is_empty() {
            Ok(Some(now))
        } else {
            Ok(runtime.next_timeout())
        }
    }

    pub fn startup(&mut self, now: Instant) -> Result<Option<Instant>> {
        assert!(
            self.rx_events.is_empty(),
            "No events received before startup"
        );

        // NOTE: use cases don't need sender as in the  runtime on startup
        //     so we don't need to set it
        let next_event = self.context.runtime_mut().next_event(now);
        assert_eq!(
            next_event, None,
            "No leftover events or timers in existing runtime on startup"
        );

        self.pipeline.startup(&self.context, now)?;
        Ok(self.context.runtime().next_timeout().cloned())
    }
}

impl<C, const BUCKET_SIZE: usize> Drop for R2Kad<C, BUCKET_SIZE> {
    fn drop(&mut self) {
        todo!("send shutdown event to  UseCases")
    }
}

// NOTE: in the future this can be refactored into a trait
//     to allow "custom" pipeline creations
struct R2KadPipeline<C, IRS, const BUCKET_SIZE: usize> {
    forward_message: ForwardProtocolMessage<C, BUCKET_SIZE>,
    random_probing: RandomOverlayDiscovery<C, BUCKET_SIZE>,
    on_disc: OverlayNeighborhoodDiscovery<C, BUCKET_SIZE>,
    vicinity_disc: VicinityDiscovery<C, BUCKET_SIZE>,
    path_probing: PathProbing<C, BUCKET_SIZE>,
    derive_forwarding_tables: DeriveFwdTableEntries<C, BUCKET_SIZE>,
    failure_handling: FailureHandling<C, BUCKET_SIZE>,
    precomputation: PrecomputePathIds<C, BUCKET_SIZE>,
    explicit_path_management: ExplicitPathManagement<C, BUCKET_SIZE>,
    inject_messages: InjectMessages<C, IRS, BUCKET_SIZE>,
    distributed_hash_table: DistributedHashTable<C, DefaultExpiringHashTable, BUCKET_SIZE>,
    distributed_hash_table_injector: DistributedHashTableInjector<C, BUCKET_SIZE>,
}

impl<C, IRS, const BUCKET_SIZE: usize> Default for R2KadPipeline<C, IRS, BUCKET_SIZE> {
    fn default() -> Self {
        let mut forward_message = ForwardProtocolMessage::default();
        let mut random_probing = RandomOverlayDiscovery::new(Default::default())
            .expect("default grouping should be valid");
        let mut on_disc = OverlayNeighborhoodDiscovery::<_, BUCKET_SIZE>::new(Default::default())
            .expect("default grouping should be valid");

        let vicinity_config = VicinityDiscoveryConfig {
            heuristic_enabled: config.heuristic_enabled,
            ..Default::default()
        };
        let mut vicinity_disc = VicinityDiscovery::new(vicinity_config);

        let mut path_probing = PathProbing::new(Default::default());
        let mut derive_forwarding_tables = DeriveFwdTableEntries::new(Default::default());
        let mut failure_handling = FailureHandling::new(Default::default());
        let mut precomputation = PrecomputePathIds::new(root_id.clone(), Default::default());
        let mut explicit_path_management = ExplicitPathManagement::new(EPMConfig::default());
        let mut distributed_hash_table: DistributedHashTable<
            _,
            DefaultExpiringHashTable,
            BUCKET_SIZE,
        > = DistributedHashTable::new(DistributedHashTableConfig::default());
        let mut inject_messages = if let Some(injection_sender) = injection_sender.as_ref() {
            let mut inject_messages =
                InjectMessages::new(InjectMessagesConfig::default(), injection_sender.clone())
                    .expect("default grouping should be valid");
            if let Err(e) = inject_messages.start(&context) {
                log::error!("Failed to start inject messages UseCase: {}", e);
                return;
            }
            Some(inject_messages)
        } else {
            None
        };
        let mut distributed_hash_table_injector = DistributedHashTableInjector::default();
    }
}
impl<C, IRS, const BUCKET_SIZE: usize> R2KadPipeline<C, IRS, BUCKET_SIZE> {
    fn startup(&mut self, context: &C, now: Instant) -> Result<()> {
        todo!();
    }

    fn process_event(&mut self, context: &C, event: UseCaseEvent) -> Result<()> {
        todo!();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use log::LevelFilter;

    pub fn init() {
        let _ = env_logger::builder()
            .filter_level(LevelFilter::Trace)
            .parse_default_env()
            .is_test(true)
            .try_init();
    }
}
