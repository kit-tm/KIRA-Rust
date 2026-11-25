//! Implementation of the protocol instance R²/KAD.

use std::{
    collections::HashMap,
    fmt::Debug,
    marker::PhantomData,
    ops::Deref,
    sync::Arc,
    time::{
        Duration,
        Instant,
    },
};

use derive_more::derive::{
    Display,
    Error,
};
use tracing::{
    Level,
    Span,
    field,
    instrument,
};

pub mod load;
mod pipeline;
pub(crate) mod runtime;

use pipeline::R2KadPipeline;
use runtime::R2KadRuntime;

#[doc(inline)]
pub use crate::domain::{
    Input,
    Output,
};
use crate::{
    context::ContextConfig,
    domain::{
        InsertionStrategy,
        NodeId,
        PathSegmentation,
        ProtocolMessageKind,
        RoutingTable,
        Timestamp,
        ULNState,
        ULNTable,
        UnderlayNeighborId,
        VicinityGraph,
        insertion_strategy::UNSStrategy,
        path::{
            InOrderCycleRemover,
            ShortestFirstPathSimplifier,
        },
        routing_table::{
            FlatRoutingTable,
            ObservableRoutingTable,
            ProtectedRoutingTable,
        },
        underlay_neighbor_table::{
            InMemoryULNTable,
            ObservableULNTable,
            observable_underlay_neighbor_table::ULNTableEvent,
        },
        vicinity::vicinity_graph::{
            ObservableVicinityGraph,
            PetVicinityGraph,
            observable_vicinity_graph::VicinityGraphEvent,
        },
    },
    r2kad::pipeline::{
        R2KadPipelineConfig,
        UseCaseStartupError,
        UseCaseStateError,
    },
    runtime::UseCaseRuntime,
    use_cases::{
        ContactEvent,
        UseCaseContext,
        VicinityEvent,
    },
};

/// Queueing delay of received messages from which a warning is logged.
const LAG_WARNING: Duration = Duration::from_millis(200);

/// Whether a received message may be dropped when it waited too long to be processed.
///
/// Lookups only: their senders retry them, and they change no state. Responses (which end
/// the timeouts of requests), probes (whose loss means a failed path) and requests changing
/// state are always processed.
fn is_sheddable(kind: ProtocolMessageKind) -> bool {
    matches!(
        kind,
        ProtocolMessageKind::FindNodeReq
            | ProtocolMessageKind::QueryRouteReq
            | ProtocolMessageKind::ULNDiscReq
            | ProtocolMessageKind::FetchReq
    )
}

pub struct Builder<C, const BUCKET_SIZE: usize> {
    context: PhantomData<C>,
    root_id: Option<NodeId>, // the node's NodeID None => random
    pipeline_config: R2KadPipelineConfig,
    current_time: Option<Instant>,
}

impl<C, const BUCKET_SIZE: usize> Builder<C, BUCKET_SIZE> {
    /// Sets the initial time of the runtime.
    ///
    /// This is required, as the protocol never reads a clock itself: the driver provides the
    /// time, e.g. the wall clock or the virtual clock of a simulator.
    pub fn current_time(mut self, now: Instant) -> Self {
        self.current_time = Some(now);
        self
    }

    /// Sets how paths to contacts are split into path segments by the forwarding tier.
    ///
    /// Has to match the capabilities of the forwarding tables in use
    /// and has to be the same on all nodes of the network.
    ///
    /// # Default
    /// [PathSegmentation::TwoSegments]
    pub fn path_segmentation(mut self, segmentation: PathSegmentation) -> Self {
        self.pipeline_config.path_segmentation = segmentation;
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
                ProtectedRoutingTable<BUCKET_SIZE, 1>,
                BUCKET_SIZE,
            >,
            UnderlayNeighborTable = ObservableULNTable<InMemoryULNTable>,
            Runtime = Arc<R2KadRuntime>,
            InsertionStrategy = UNSStrategy<
                ObservableRoutingTable<ProtectedRoutingTable<BUCKET_SIZE, 1>, BUCKET_SIZE>,
                InOrderCycleRemover,
                ShortestFirstPathSimplifier,
                BUCKET_SIZE,
            >,
            VicinityGraph = ObservableVicinityGraph<PetVicinityGraph>,
        >,
{
    /// Builds the protocol instance.
    ///
    /// # Panics
    ///
    /// If [current_time](Self::current_time) was not set.
    pub fn build(&self) -> R2Kad<C, BUCKET_SIZE> {
        let now = self
            .current_time
            .expect("Builder::current_time has to be set, the protocol doesn't read a clock");
        let _clock = Timestamp::set_clock(now);
        let root_id = self.root_id.unwrap_or_else(NodeId::random);
        let runtime = Arc::new(R2KadRuntime::with_startup_time(now));
        let pipeline = R2KadPipeline::new(self.pipeline_config);

        let mut routing_table = ObservableRoutingTable::from(ProtectedRoutingTable::from(
            FlatRoutingTable::new(root_id).expect("FlatRoutingTable parameters should be valid"),
        ));
        {
            // this is why Arc<R2KadRuntime> is required
            let runtime = runtime.clone();
            routing_table.add_observer(move |event| {
                use crate::domain::routing_table::observable_routing_table::RoutingTableEvent::*;
                let contact_event = match event {
                    NewContact(contact) => ContactEvent::New(contact),
                    RemovedContact(contact) => ContactEvent::Removed(contact),
                    UpdatedContact { new, old } => ContactEvent::Updated { new, old },
                    UpdatedBucket(bucket) => ContactEvent::BucketUpdated(bucket),
                    NewBucket(bucket) => ContactEvent::NewBucket(bucket),
                };
                runtime.broadcast_event(Box::new(contact_event));
            });
        }
        routing_table.add_observer(|event| log::trace!(target: "routing_table", "{event}"));

        let mut vicinity_graph = ObservableVicinityGraph::from(PetVicinityGraph::new(root_id));
        {
            let runtime = runtime.clone();
            vicinity_graph.add_observer(move |event| {
                let vicinity_event = match event {
                    VicinityGraphEvent::Removed(node) => VicinityEvent::Removed(node),
                };
                runtime.broadcast_event(vicinity_event)
            });
        }
        vicinity_graph.add_observer(|event| log::trace!(target: "vicinity_graph", "{event}"));

        let mut uln_table = ObservableULNTable::<InMemoryULNTable>::default();
        {
            let runtime = runtime.clone();
            uln_table.add_observer(move |event| {
                let vicinity_event = match event {
                    ULNTableEvent::SSNChanged => VicinityEvent::SSNChanged,
                };
                runtime.broadcast_event(vicinity_event)
            });
        }

        let insertion_strategy = UNSStrategy::new(InOrderCycleRemover, ShortestFirstPathSimplifier);

        let context = C::new(ContextConfig {
            root_id,
            routing_table,
            runtime,
            insertion_strategy,
            uln_table,
            vicinity_graph,
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
            current_time: None,
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
/// ```no_run
/// use std::time::Instant;
///
/// use kira_r2kad::context::SyncContext;
/// use kira_r2kad::{Output, R2Kad};
///
/// // create R²/KAD protocol instance with random NodeId,
/// // the driver provides the time (here the wall clock)
/// let mut r2kad = R2Kad::<SyncContext<_, _, _, _, _>, 20>::builder()
///     .current_time(Instant::now())
///     .build();
///
/// // startup R²/KAD instance
/// {
///     let now = Instant::now();
///     r2kad.startup(now).unwrap();
/// }
///
/// # tokio::runtime::Builder::new_multi_thread()
/// #   .enable_all()
/// #   .build()
/// #   .unwrap()
/// #   .block_on(async {
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
///         Some(input) = async move {
///           todo!("Receive Input events like ProtocolMessages from remote peers")
///         } => {
///             let now = Instant::now();
///             r2kad.handle_input(input, now).unwrap();
///         }
///         _ = tokio::time::sleep_until(timer_due.into()) => {
///             let now = Instant::now();
///             r2kad.handle_timeout(now).unwrap();
///         }
///     }
/// }
/// # });
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
    /// Creates a new [R2Kad] instance with the default use case pipeline employed.
    pub fn new(context: C) -> Self {
        let pipeline = R2KadPipeline::default();

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
        ULNTable + Deref<Target = HashMap<NodeId, (UnderlayNeighborId, ULNState)>> + Debug,
    for<'a> C::RoutingTable: RoutingTable<'a, BUCKET_SIZE> + Debug,
    C::InsertionStrategy: InsertionStrategy<C::RoutingTable, C::UnderlayNeighborTable, BUCKET_SIZE>,
    C::VicinityGraph: VicinityGraph + Debug,
{
    /// Startup the protocol instance.
    #[instrument(level = Level::DEBUG, target = "r2kad", skip_all)]
    pub fn startup(&mut self, now: Instant) -> Result<()> {
        let _clock = Timestamp::set_clock(now);
        log::trace!(target: "r2kad", "startup instance");
        self.context.runtime().set_current_time(now);
        self.pipeline.startup(&self.context)?;
        Ok(())
    }

    /// Process received [Input] event.
    #[instrument(level = Level::DEBUG, target = "r2kad", skip_all, fields(reason = ?received_event))]
    pub fn handle_input(&mut self, received_event: Input, now: Instant) -> Result<()> {
        self.handle_received(received_event, now, now)
    }

    /// Handles an input received at `received_at` (e.g., a protocol message that waited in
    /// the receive buffer of a socket) at `now`.
    ///
    /// Timers due before `received_at` fire before the input, the others after it: a response
    /// that arrived in time cancels the timeout of its request, also when the node processes
    /// it late. The time the input waited is the queueing delay that lengthens the timeouts of
    /// requests (see [load](self::load)).
    pub fn handle_received(
        &mut self,
        received_event: Input,
        received_at: Instant,
        now: Instant,
    ) -> Result<()> {
        // within the time already handled and now
        let received_at = received_at
            .max(self.context.runtime().current_time())
            .min(now);
        self.handle_timeout(received_at)?;

        let runtime = self.context.runtime();
        runtime.record_queueing_delay(now - received_at);
        let lag = runtime.queueing_delay();
        if lag > LAG_WARNING {
            crate::rate_limited!(Duration::from_secs(10), |suppressed| tracing::warn!(
                target: "r2kad",
                ?lag,
                suppressed,
                "Processing lags behind: received messages wait before they are handled"
            ));
        }

        let _clock = Timestamp::set_clock(now);
        self.context.runtime().set_current_time(now);
        debug_assert_eq!(self.context.runtime().next_event(), None);

        if let Input::Message(message, _) = &received_event
            && is_sheddable(message.kind())
            && now - received_at > self.context.runtime().stale_request_age()
        {
            let kind = message.kind();
            crate::rate_limited!(Duration::from_secs(10), |suppressed| tracing::info!(
                target: "r2kad",
                ?kind,
                waited = ?(now - received_at),
                suppressed,
                "Dropping a lookup whose sender has given up on it"
            ));
            return self.handle_timeout(now);
        }

        // handle input event as UseCaseEvent
        self.pipeline
            .process_event(&self.context, received_event.into())?;

        // consume __all__ events generated inside the runtime because of this
        while let Some(event) = self.context.runtime().next_event() {
            self.pipeline.process_event(&self.context, event)?;
        }

        // timers that became due while the input waited
        self.handle_timeout(now)
    }

    /// Handle a timeout.
    ///
    /// The next time this method has to be called can be obtained
    /// with [poll_timeout](Self::poll_timeout).
    #[instrument(level = Level::DEBUG, target = "r2kad", skip_all, fields(reason = field::Empty))]
    pub fn handle_timeout(&mut self, now: Instant) -> Result<()> {
        let _clock = Timestamp::set_clock(now);
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
