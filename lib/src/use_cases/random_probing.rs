use std::error::Error;
use std::fmt::{Display, Formatter};
use std::marker::PhantomData;
use std::time::Duration;

use crate::context::Context;
use crate::domain::{Contact, RoutingTable};
use crate::messaging::{
    FindNodeReqData, Nonce, ProtocolMessageSender, RTableReqType, ReqRspMessage,
};
use crate::runtime::Runtime;
use crate::use_cases::{TimerId, UseCase, UseCaseEvent, UseCaseState};

#[derive(Debug, Copy, Clone)]
pub struct RandomProbingConfig {
    pub timeout: Duration,
    pub neighborhood_size: usize,
}

impl Default for RandomProbingConfig {
    fn default() -> Self {
        Self {
            // Default: 2.5 Messages/s => 1000 ms / 2.5 = 400 ms
            timeout: Duration::from_millis(400),
            neighborhood_size: 20,
        }
    }
}

/// Probe random contacts to populate or improve all buckets with existing contacts.
#[derive(Debug)]
pub struct RandomProbingUseCase<C, const BUCKET_SIZE: usize> {
    config: RandomProbingConfig,
    state: RandomProbingState,
    _c: PhantomData<C>,
}

impl<C, const BUCKET_SIZE: usize> RandomProbingUseCase<C, BUCKET_SIZE> {
    pub fn new(config: RandomProbingConfig) -> Self {
        Self {
            config,
            state: RandomProbingState::Initialized,
            _c: PhantomData::default(),
        }
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase<C> for RandomProbingUseCase<C, BUCKET_SIZE>
where
    C: Context,
    C::Runtime: Runtime,
    C::MessageSender: ProtocolMessageSender,
    C::RoutingTable: RoutingTable<BUCKET_SIZE>,
    for<'b> &'b C::RoutingTable: IntoIterator<Item = &'b Contact>,
{
    type Error = RandomProbingError;
    type State = RandomProbingState;

    fn start(&mut self, context: &C) -> Result<(), Self::Error> {
        let timer_id = context
            .runtime()
            .register_periodic_timer(self.config.timeout);

        self.state = RandomProbingState::Running(timer_id);

        Ok(())
    }

    fn handle_event(&mut self, context: &C, event: UseCaseEvent) -> Result<(), Self::Error> {
        if let (UseCaseEvent::Timer(event_id), RandomProbingState::Running(timer_id)) =
            (event, &self.state)
        {
            if &event_id == timer_id {
                let random_id = context
                    .routing_table()
                    .random_id()
                    .cloned()
                    .ok_or(RandomProbingError::EmptyRoutingTable)?;

                let message = ReqRspMessage {
                    nonce: Nonce::random(),
                    source: context.root_id().clone(),
                    destination: random_id,
                    data: FindNodeReqData {
                        req_type: RTableReqType::NeighborHood(self.config.neighborhood_size),
                        exact: false,
                    },
                };

                if let Err(e) = context.message_sender_mut().send(message) {
                    log::error!("MessageSender failed: {}", e);
                    self.state = RandomProbingState::Error;
                    return Err(RandomProbingError::SendFailed);
                }
            }
        }

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}

#[derive(Debug)]
pub enum RandomProbingError {
    SendFailed,
    EmptyRoutingTable,
}

impl Display for RandomProbingError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SendFailed => write!(f, "Failed to send message"),
            Self::EmptyRoutingTable => write!(f, "Could not probe, routing table is empty"),
        }
    }
}

impl Error for RandomProbingError {}

#[derive(Debug, Eq, PartialEq)]
pub enum RandomProbingState {
    Initialized,
    Running(TimerId),
    Error,
}

impl UseCaseState for RandomProbingState {
    /// Random probing will never be finished.
    fn is_finished(&self) -> bool {
        false
    }

    fn is_error(&self) -> bool {
        self == &Self::Error
    }
}
