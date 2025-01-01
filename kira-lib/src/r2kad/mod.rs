//! Implementation of the protocol instance R²/KAD.

use std::{collections::VecDeque, time::Instant};
use thiserror::Error;

mod pipeline;
use pipeline::R2KadPipeline;

#[doc(inline)]
pub use crate::domain::protocol_event::{Input, Output};

use crate::{domain::NodeId, use_cases::UseCaseContext};

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
            rx_events: VecDeque::default(),
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
    C: UseCaseContext,
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
    /// In all three cases the method should be called immediately after new
    /// data is received with [receive_event](Self::receive_event).
    ///
    /// The method can be called after the returned [Instant] is in the past.
    pub fn process_received(&mut self, now: Instant) -> Result<Option<Instant>> {
        let mut runtime = self.context.runtime_mut();

        // only take one use case event
        if let Some(event) = self.rx_events.pop_front() {
            runtime.spawn_event(event);
        }

        // consume __all__ events generated inside the runtime
        while let Some(event) = runtime.next_event(now) {
            self.pipeline.process_event(&self.context, event)?;
        }

        if self.rx_events.is_empty() {
            Ok(runtime.next_timeout().cloned())
        } else {
            Ok(Some(now))
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
