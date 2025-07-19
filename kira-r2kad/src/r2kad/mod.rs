//! Implementation of the protocol instance R²/KAD.

use derive_more::derive::{Display, Error};
use std::{
    collections::{HashMap, HashSet},
    marker::PhantomData,
    ops::Deref,
    sync::Arc,
    time::Instant,
};
use tracing::{field, instrument, Level, Span};

mod pipeline;
pub(crate) mod runtime;
use pipeline::R2KadPipeline;

#[doc(inline)]
pub use crate::domain::protocol_event::{Input, Output};

use crate::{
    context::ContextConfig,
    domain::{
        observable_routing_table::ObservableRoutingTable,
        unlimited_uln_routing_table::UnlimitedULNRoutingTable, FlatRoutingTable, InMemoryULNTable,
        InOrderCycleRemover, InsertionStrategy, NodeId, RoutingTable, ShortestFirstPathSimplifier,
        ULNTable, UNSStrategy, UnderlayNeighborId,
    },
    r2kad::pipeline::{R2KadPipelineConfig, UseCaseStartupError, UseCaseStateError},
    runtime::UseCaseRuntime,
    use_cases::{ContactEvent, UseCaseContext},
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
        RoutingTable = ObservableRoutingTable<
            UnlimitedULNRoutingTable<BUCKET_SIZE, 1>,
            BUCKET_SIZE,
        >,
        UnderlayNeighborTable = InMemoryULNTable,
        Runtime = Arc<R2KadRuntime>,
        InsertionStrategy = UNSStrategy<
            ObservableRoutingTable<UnlimitedULNRoutingTable<BUCKET_SIZE, 1>, BUCKET_SIZE>,
            InOrderCycleRemover,
            ShortestFirstPathSimplifier,
            BUCKET_SIZE,
        >,
    >,
{
    pub fn build(&self) -> R2Kad<C, BUCKET_SIZE> {
        let root_id = self.root_id.unwrap_or_else(NodeId::random);
        let runtime = Arc::new(R2KadRuntime::with_startup_time(self.current_time));
        let pipeline = R2KadPipeline::new(self.pipeline_config, &root_id);

        let mut routing_table = ObservableRoutingTable::from(UnlimitedULNRoutingTable::from(
            FlatRoutingTable::new(root_id).expect("FlatRoutingTable parameters should be valid"),
        ));

        // Add observer which emits to runtime
        {
            // this is why Arc<R2KadRuntime> is required
            let runtime = runtime.clone();
            routing_table.add_observer(move |event| {
                use crate::domain::observable_routing_table::RoutingTableEvent::*;
                let contact_event = match event {
                    NewContact(contact) => ContactEvent::New(contact),
                    RemovedContact(contact) => ContactEvent::Removed(contact),
                    UpdatedContact { new, old } => ContactEvent::Updated { new, old },
                    UpdatedBucket(bucket) => ContactEvent::BucketUpdated(bucket),
                    NewBucket(bucket) => ContactEvent::NewBucket(bucket),
                };
                runtime.broadcast_event(contact_event);
            });
        }
        routing_table.add_observer(|event| log::trace!(target: "routing_table", "{event}"));

        let insertion_strategy = UNSStrategy::new(InOrderCycleRemover, ShortestFirstPathSimplifier);

        let context = C::new(ContextConfig {
            root_id,
            routing_table,
            runtime,
            insertion_strategy,
            uln_table: InMemoryULNTable::new(),
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
/// use kira_lib::context::SyncContext;
/// use kira_lib::{Output, R2Kad};
///
/// // create R²/KAD protocol instance with random NodeId
/// let mut r2kad = R2Kad::<SyncContext<_, _, _, _>, 20>::builder().build();
///
/// // startup R²/KAD instance
/// {
///     let now = Instant::now();
///     r2kad.startup(now).unwrap();
/// }
///
/// loop {
///     // 1. Process protocol instance output
///     while let Some(output) = r2kad.poll_output() {
///         match output {
///             Output::SendProtocolMessage(message, destination) => {
///                 todo!("Send data to remote peer.");
///             }
///             Output::UpdateForwardingTables(update_req) => {
///                 todo!("Update the forwarding tables.");
///             }
///         }
///     }
///
///     let timer_due = r2kad.poll_timeout().unwrap();
///
///     // 2. Drive protocol instance progress
///     tokio::select! {
///         biased; // poll in order since we check timers on handling input regardlessly
///
///         Some(input) = async move { todo!("Receive Input events like ProtocolMessages from remote peers")} => {
///             let now = Instant::now();
///             r2kad.handle_input(input, now).unwrap();
///         }
///         _ = tokio::time::sleep_until(timer_due.into()) => {
///             let now = Instant::now();
///             r2kad.handle_timeout(now).unwrap();
///         }
///     }
/// }
/// ```
#[derive(Debug)]
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

    pub fn context(&self) -> &C {
        &self.context
    }
}

impl<C, const BUCKET_SIZE: usize> R2Kad<C, BUCKET_SIZE>
where
    C: UseCaseContext,
    C::Runtime: Deref<Target = R2KadRuntime>,
    C::UnderlayNeighborTable:
        ULNTable + Deref<Target = HashMap<NodeId, UnderlayNeighborId>> + std::fmt::Debug,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE> + std::fmt::Debug,
    C::InsertionStrategy: InsertionStrategy<C::RoutingTable, C::UnderlayNeighborTable, BUCKET_SIZE>,
{
    /// Startup the protocol instance.
    #[instrument(level = Level::DEBUG, target = "r2kad", skip_all)]
    pub fn startup(&mut self, now: Instant) -> Result<()> {
        log::trace!(target: "r2kad", "startup instance");
        self.context.runtime().set_current_time(now);
        self.pipeline.startup(&self.context)?;
        Ok(())
    }

    /// Process received [Input] event.
    #[instrument(level = Level::DEBUG, target = "r2kad", skip_all, fields(reason = ?received_event))]
    pub fn handle_input(&mut self, received_event: Input, now: Instant) -> Result<()> {
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
    #[instrument(level = Level::DEBUG, target = "r2kad", skip_all, fields(reason = field::Empty))]
    pub fn handle_timeout(&mut self, now: Instant) -> Result<()> {
        self.context.runtime().set_current_time(now);

        let mut reason = true;
        // process timer events first
        while let Some(due_timer) = self.context.runtime().next_timer() {
            log::trace!(target: "r2kad", "handle timer: {due_timer:?}");

            // "first" timer as reason
            if reason {
                Span::current().record("reason", format!("{due_timer}"));
                reason = false;
            }

            let timer_event = crate::use_cases::UseCaseEvent::Timer(due_timer);
            self.pipeline.process_event(&self.context, timer_event)?;
        }

        // don't need to check timers again because
        // even if a UseCase starts a new timer at this point
        // timers can only be due in the future (positive duration)

        // consume __all__ events generated inside the runtime
        while let Some(event) = self.context.runtime().next_event() {
            log::trace!(target: "r2kad", "handle follow up event: {event:?}");
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
