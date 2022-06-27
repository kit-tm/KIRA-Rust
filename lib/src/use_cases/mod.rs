use std::error::Error;
use std::ops::Deref;

use crate::messaging::messages::ProtocolMessage;

pub mod bootstrap;
pub mod pn_probing;
pub mod random_probing;

#[derive(Debug, Clone)]
pub enum UseCaseEvent {
    Message(ProtocolMessage),
    Timer(TimerId),
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub struct TimerId(usize);

impl From<usize> for TimerId {
    fn from(value: usize) -> Self {
        Self(value)
    }
}

impl Deref for TimerId {
    type Target = usize;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub trait UseCaseState {
    fn is_finished(&self) -> bool;
    fn is_error(&self) -> bool;
}

pub trait UseCase<C> {
    type Error: Error + Sized;
    type State: UseCaseState + Sized;

    fn start(&mut self, context: &C) -> Result<(), Self::Error>;
    fn handle_event(&mut self, context: &C, event: UseCaseEvent) -> Result<(), Self::Error>;
    fn state(&self) -> &Self::State;
}
