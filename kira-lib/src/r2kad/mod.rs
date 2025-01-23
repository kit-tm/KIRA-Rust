//! Implementation of the protocol instance R²/KAD.

use derive_more::derive::{Display, Error};
use std::{
    collections::{HashMap, HashSet},
    marker::PhantomData,
    ops::Deref,
    time::Instant,
};

mod pipeline;
pub(crate) mod runtime;
use pipeline::R2KadPipeline;

#[doc(inline)]
pub use crate::domain::protocol_event::{Input, Output};

use crate::{
    context::ContextConfig,
    domain::{
        observable_routing_table::ObservableRoutingTable,
        unlimited_pn_routing_table::UnlimitedPNRoutingTable, FlatRoutingTable, InMemoryPNTable,
        InOrderCycleRemover, InsertionStrategy, NodeId, PNSStrategy, RoutingTable,
        ShortestFirstPathSimplifier, UNTable, UnderlayNeighborId,
    },
    r2kad::pipeline::{R2KadPipelineConfig, UseCaseStartupError, UseCaseStateError},
    runtime::UseCaseRuntime,
    use_cases::UseCaseContext,
};
use runtime::R2KadRuntime;

pub struct Builder<C, const BUCKET_SIZE: usize> {
    context: PhantomData<C>,
    root_id: Option<NodeId>, // None => random
    pipeline_config: R2KadPipelineConfig,
    current_time: Instant,
}

impl<C, const BUCKET_SIZE: usize> Builder<C, BUCKET_SIZE> {
    /// Enables heuristics.
    // TODO: document what it actually means.
    pub fn enable_heuristics(mut self) -> Self {
        self.pipeline_config.heuristic_enabled = true;
        self
    }

    /// Sets the initial time of the runtime.
    pub fn current_time(mut self, now: Instant) -> Self {
        self.current_time = now;
        self
    }

    /// Set [NodeId] of protocol instance.
    ///
    /// # Default
    /// Random [NodeId].
    pub fn root_id(mut self, root_id: NodeId) -> Self {
        self.root_id = Some(root_id);
        self
    }
}

impl<C, const BUCKET_SIZE: usize> Builder<C, BUCKET_SIZE>
where
    C: UseCaseContext<
        RoutingTable = ObservableRoutingTable<UnlimitedPNRoutingTable<BUCKET_SIZE, 1>, BUCKET_SIZE>,
        PhysicalNeighborTable = InMemoryPNTable,
        Runtime = R2KadRuntime,
        InsertionStrategy = PNSStrategy<
            ObservableRoutingTable<UnlimitedPNRoutingTable<BUCKET_SIZE, 1>, BUCKET_SIZE>,
            InOrderCycleRemover,
            ShortestFirstPathSimplifier,
            BUCKET_SIZE,
        >,
    >,
{
    pub fn build(&self) -> R2Kad<C, BUCKET_SIZE> {
        let root_id = self.root_id.unwrap_or_else(NodeId::random);
        let runtime = R2KadRuntime::with_startup_time(self.current_time);
        let pipeline = R2KadPipeline::new(self.pipeline_config, &root_id);

        let routing_table = ObservableRoutingTable::from(UnlimitedPNRoutingTable::from(
            FlatRoutingTable::new(root_id).expect("FlatRoutingTable parameters should be valid"),
        ));

        // TODO: add routing table observers that broadcast contact updates
        // Rc<RefCell<Vec<ContactUpdates>>> in R2Kad and ObservableRoutingTable

        let insertion_strategy = PNSStrategy::new(InOrderCycleRemover, ShortestFirstPathSimplifier);

        let context = C::new(ContextConfig {
            root_id,
            routing_table,
            runtime,
            insertion_strategy,
            pn_table: InMemoryPNTable::new(),
            not_via: HashSet::default(),
        });

        R2Kad { context, pipeline }
    }
}

impl<C, const BUCKET_SIZE: usize> Default for Builder<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self {
            context: Default::default(),
            root_id: None,
            pipeline_config: Default::default(),
            current_time: Instant::now(),
        }
    }
}

#[derive(Debug, Display, Error)]
#[non_exhaustive]
pub enum R2KadError {
    #[display("Handling an event resulted in an invalid protocol state")]
    StateError,
    #[display("Starting up the protocol instance failed")]
    StartupError,
}

impl From<UseCaseStateError> for R2KadError {
    fn from(_value: UseCaseStateError) -> Self {
        Self::StateError
    }
}

impl From<UseCaseStartupError> for R2KadError {
    fn from(_value: UseCaseStartupError) -> Self {
        Self::StateError
    }
}

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
    context: C,
    pipeline: R2KadPipeline<C, BUCKET_SIZE>,
}

impl<C: UseCaseContext, const BUCKET_SIZE: usize> R2Kad<C, BUCKET_SIZE> {
    pub fn builder() -> Builder<C, BUCKET_SIZE> {
        Builder::default()
    }
}

impl<C: UseCaseContext, const BUCKET_SIZE: usize> R2Kad<C, BUCKET_SIZE> {
    pub fn new(context: C) -> Self {
        let root = NodeId::random();
        Self::with_root(root, context)
    }

    pub fn with_root(root: NodeId, context: C) -> Self {
        let pipeline = R2KadPipeline::new(Default::default(), &root);

        Self { context, pipeline }
    }
}

impl<C, const BUCKET_SIZE: usize> R2Kad<C, BUCKET_SIZE>
where
    C: UseCaseContext<Runtime = R2KadRuntime>,
    C::Runtime: UseCaseRuntime,
    C::PhysicalNeighborTable:
        UNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>> + std::fmt::Debug,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE> + std::fmt::Debug,
    C::InsertionStrategy: InsertionStrategy<C::RoutingTable, C::PhysicalNeighborTable, BUCKET_SIZE>,
{
    /// Startup the protocol instance.
    pub fn startup(&mut self, now: Instant) -> Result<()> {
        self.context.runtime().set_current_time(now);
        self.pipeline.startup(&self.context)?;
        Ok(())
    }

    /// Process received [Input] event.
    pub fn handle_input(&mut self, received_event: Input, now: Instant) -> Result<()> {
        log::trace!("Handling input: {:?}", received_event);

        // just to be save we check for due timers
        self.handle_timeout(now)?;

        debug_assert_eq!(self.context.runtime().next_event(), None);

        // handle input event as UseCaseEvent
        self.pipeline
            .process_event(&self.context, received_event.into())?;

        // consume __all__ events generated inside the runtime because of this
        while let Some(event) = self.context.runtime().next_event() {
            self.pipeline.process_event(&self.context, event)?;
        }

        Ok(())
    }

    /// Handle a timeout.
    ///
    /// The next time this method has to be called can be obtained
    /// with [poll_timeout](Self::poll_timeout).
    pub fn handle_timeout(&mut self, now: Instant) -> Result<()> {
        log::trace!("Handling timeout");

        self.context.runtime().set_current_time(now);

        // process timer events first
        while let Some(due_timer) = self.context.runtime().next_timer() {
            let timer_event = crate::use_cases::UseCaseEvent::Timer(due_timer);
            self.pipeline.process_event(&self.context, timer_event)?;
        }

        // don't need to check timers again because
        // even if a UseCase starts a new timer at this point
        // timers can only be due in the future (positive duration)

        // consume __all__ events generated inside the runtime
        while let Some(event) = self.context.runtime().next_event() {
            self.pipeline.process_event(&self.context, event)?;
        }

        Ok(())
    }

    /// [Output] events of the protocol.
    pub fn poll_output(&mut self) -> Option<Output> {
        self.context.runtime().poll_output()
    }

    /// Next time [handle_timeout](Self::handle_timeout) should be called.
    pub fn poll_timeout(&mut self) -> Option<Instant> {
        self.context.runtime().poll_timeout()
    }
}
