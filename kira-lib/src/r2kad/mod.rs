//! Implementation of the protocol instance R²/KAD.

pub mod runtime;

use std::{collections::VecDeque, time::Instant};
use thiserror::Error;

#[doc(inline)]
pub use crate::domain::protocol_event::{Input, Output};

use crate::{
    context::UseCaseContext,
    domain::NodeId,
    use_cases::{
        derive_fwd_table_entries::DeriveFwdTableEntries,
        distributed_hash_table::{DefaultExpiringHashTable, DistributedHashTable},
        distributed_hash_table_injector::DistributedHashTableInjector,
        explicit_path_management::ExplicitPathManagement,
        failure_handling::FailureHandling,
        forward_protocol_message::ForwardProtocolMessage,
        //inject_messages::InjectMessages,
        overlay_neighborhood_discovery::OverlayNeighborhoodDiscovery,
        path_probing::PathProbing,
        precompute_paths_and_path_ids::PrecomputePathIds,
        random_overlay_discovery::RandomOverlayDiscovery,
        vicinity_discovery::{VicinityDiscovery, VicinityDiscoveryConfig},
        UseCaseEvent,
    },
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
        let pipeline = R2KadPipeline::new(Default::default(), &root);

        Ok(Self {
            root,
            rx_events: Vec::default(),
            // TODO: implement context builder
            context,
            pipeline,
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

impl<C, const BUCKET_SIZE: usize> R2Kad<C, BUCKET_SIZE>
where
    C: UseCaseContext<Runtime = UseCaseRuntime>,
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
    pub fn process_received(&mut self, now: Instant) -> Result<Option<Instant>> {
        let mut runtime = self.context.runtime_mut();

        // only take one use case event
        if let Some(event) = self.rx_events.pop_front() {
            runtime.spawn_event(event.into());
        }

        // consume __all__ events generated inside the runtime
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

#[derive(Debug, Clone, Default)]
struct R2KadPipelineConfig {
    pub heuristic_enabled: bool,
    //pub benchmark_path: Option<BufWriter<File>>,
}

// NOTE: in the future this can be refactored into a trait
//     to allow "custom" pipeline creations
struct R2KadPipeline<C, const BUCKET_SIZE: usize> {
    forward_message: ForwardProtocolMessage<C, BUCKET_SIZE>,
    random_probing: RandomOverlayDiscovery<C, BUCKET_SIZE>,
    on_disc: OverlayNeighborhoodDiscovery<C, BUCKET_SIZE>,
    vicinity_disc: VicinityDiscovery<C, BUCKET_SIZE>,
    path_probing: PathProbing<C, BUCKET_SIZE>,
    derive_forwarding_tables: DeriveFwdTableEntries<C, BUCKET_SIZE>,
    failure_handling: FailureHandling<C, BUCKET_SIZE>,
    precomputation: PrecomputePathIds<C, BUCKET_SIZE>,
    explicit_path_management: ExplicitPathManagement<C, BUCKET_SIZE>,
    distributed_hash_table: DistributedHashTable<C, DefaultExpiringHashTable, BUCKET_SIZE>,
    distributed_hash_table_injector: DistributedHashTableInjector<C, BUCKET_SIZE>,
    //inject_messages: InjectMessages<C, IRS, BUCKET_SIZE>,
}

impl<C, const BUCKET_SIZE: usize> R2KadPipeline<C, BUCKET_SIZE> {
    pub fn new(config: R2KadPipelineConfig, root_id: &NodeId) -> Self {
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
        let mut explicit_path_management = ExplicitPathManagement::new(Default::default());
        let mut distributed_hash_table: DistributedHashTable<
            _,
            DefaultExpiringHashTable,
            BUCKET_SIZE,
        > = DistributedHashTable::new(Default::default());
        let mut distributed_hash_table_injector = DistributedHashTableInjector::default();

        //let mut inject_messages = if let Some(injection_sender) = injection_sender.as_ref() {
        //    let mut inject_messages =
        //        InjectMessages::new(Default::default(), injection_sender.clone())
        //            .expect("default grouping should be valid");
        //    if let Err(e) = inject_messages.start(&context) {
        //        log::error!("Failed to start inject messages UseCase: {}", e);
        //        return;
        //    }
        //    Some(inject_messages)
        //} else {
        //    None
        //};

        Self {
            forward_message,
            random_probing,
            on_disc,
            vicinity_disc,
            path_probing,
            derive_forwarding_tables,
            failure_handling,
            precomputation,
            explicit_path_management,
            distributed_hash_table,
            distributed_hash_table_injector,
        }
    }
}

impl<C, const BUCKET_SIZE: usize> R2KadPipeline<C, BUCKET_SIZE> {
    fn startup(&mut self, context: &C, now: Instant) -> Result<()> {
        todo!("Implement R2KadPipeline::startup");
    }

    fn process_event(&mut self, context: &C, event: UseCaseEvent) -> Result<()> {
        todo!("Implement R2KadPipeline::process_event");
    }
}
