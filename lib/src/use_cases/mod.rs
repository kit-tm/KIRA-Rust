use std::error::Error;
use std::ops::Deref;
use crate::domain::Port;

use crate::messaging::messages::ProtocolMessage;

pub mod bootstrap;
pub mod handle_hello;
pub mod pn_probing;
pub mod random_probing;

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum UseCaseEvent {
    Message(ProtocolMessage, Port),
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

pub trait UseCase {
    type Context;
    type Error: Error + Sized;
    type State: UseCaseState + Sized;

    fn start(&mut self, context: &Self::Context) -> Result<(), Self::Error>;
    fn handle_event(
        &mut self,
        context: &Self::Context,
        event: UseCaseEvent,
    ) -> Result<(), Self::Error>;
    fn state(&self) -> &Self::State;
}
