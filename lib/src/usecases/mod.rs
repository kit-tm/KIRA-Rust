use crate::messaging::Message;
use std::ops::Deref;

pub mod bootstrap;

#[derive(Debug, Clone)]
pub enum UseCaseEvent<const ID_SIZE: usize> {
    Message(Message<ID_SIZE>),
    Timer(TimerId),
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
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
