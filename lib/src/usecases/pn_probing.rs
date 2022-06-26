use std::error::Error;
use std::fmt::{Display, Formatter};
use std::marker::PhantomData;
use std::time::Duration;

use crate::context::Context;
use crate::domain::NodeId;
use crate::messaging::{HelloMessage, ProtocolMessageSender};
use crate::runtime::Runtime;
use crate::usecases::{State, TimerId, UseCase, UseCaseEvent};

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
pub struct PNProbingConfig {
    pub probing_timeout: Duration,
}

impl Default for PNProbingConfig {
    fn default() -> Self {
        Self {
            // TODO: Useful timeout duration?
            probing_timeout: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum PNProbingState {
    Initialized,
    Running(TimerId),
    Finished,
    Error,
}

impl State for PNProbingState {
    fn is_finished(&self) -> bool {
        self == &Self::Finished
    }

    fn is_error(&self) -> bool {
        self == &Self::Error
    }
}

#[derive(Debug)]
pub enum PNProbingError {
    SendError,
}

impl Display for PNProbingError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SendError => write!(f, "Sending a message failed"),
        }
    }
}

impl Error for PNProbingError {}

#[derive(Debug, Clone)]
pub struct PNProbingUseCase<C, const BUCKET_SIZE: usize> {
    _c: PhantomData<C>,
    state: PNProbingState,
    config: PNProbingConfig,
}

impl<C, const BUCKET_SIZE: usize> Default for PNProbingUseCase<C, BUCKET_SIZE> {
    fn default() -> Self {
        Self::new(PNProbingConfig::default())
    }
}

impl<C, const BUCKET_SIZE: usize> PNProbingUseCase<C, BUCKET_SIZE> {
    pub fn new(config: PNProbingConfig) -> Self {
        Self {
            _c: PhantomData::default(),
            state: PNProbingState::Initialized,
            config,
        }
    }
}

impl<C, const BUCKET_SIZE: usize> UseCase<C> for PNProbingUseCase<C, BUCKET_SIZE>
where
    C: Context,
    C::Runtime: Runtime,
    C::MessageSender: ProtocolMessageSender,
{
    type Error = PNProbingError;
    type State = PNProbingState;

    fn start(&mut self, context: &C) -> Result<(), Self::Error> {
        let timer_id = context
            .runtime()
            .register_timer(self.config.probing_timeout);

        self.state = PNProbingState::Running(timer_id);

        Ok(())
    }

    fn handle_event(&mut self, context: &C, event: UseCaseEvent) -> Result<(), Self::Error> {
        if let (UseCaseEvent::Timer(id), PNProbingState::Running(timer_id)) =
            (event, self.state.clone())
        {
            if id == timer_id {
                if let Err(e) = context.message_sender_mut().send(HelloMessage {
                    source: context.root_id().clone(),
                    destination: NodeId::zero(),
                }) {
                    log::error!("MessageSender failed: {}", e);
                    self.state = PNProbingState::Error;
                    return Err(PNProbingError::SendError);
                }
            }
        }

        Ok(())
    }

    fn state(&self) -> &Self::State {
        &self.state
    }
}
