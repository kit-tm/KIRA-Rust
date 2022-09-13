use std::error::Error;
use std::fmt::{Display, Formatter};
use std::ops::Deref;

use crate::domain::{Contact, Port};
use crate::messaging::messages::ProtocolMessage;

pub mod forward_protocol_message;
pub mod handle_hello;
pub mod handle_pn_advertising;
pub mod handle_vicinity_discovery;
pub mod message_info_extraction;
pub mod overlay_neighborhood_discovery;
pub mod periodic_pn_advertising;
pub mod random_probing;
pub mod vicinity_discovery;

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum UseCaseEvent {
    Message(ProtocolMessage, Port),
    Timer(TimerId),
    Contact(ContactEvent),
}

/// Contact Events which can be handled by UseCases.
///
/// This is only a subset of [RoutingTableEvent] as a UseCase should not react to
/// all kinds of [RoutingTableEvent]s.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ContactEvent {
    New(Contact),
    Updated { new: Contact, old: Contact },
    Removed(Contact),
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
    fn is_error(&self) -> bool;
}

/// A default [UseCaseState] implementation for a [UseCase] which doesn't have timers
/// or other states.
///
/// The opposite of a reactive [UseCase] is the active [UseCase] which has more
/// states and e.g. creates timers.
///
/// A reactive [UseCase] is either idle and waits for incoming events or is in
/// unrecoverable error state.
#[derive(Debug, Eq, PartialEq, Clone, Default)]
pub enum ReactiveUseCaseState {
    /// The [UseCase] is waiting for incoming events.
    #[default]
    Idle,
    /// The [UseCase] reached an unrecoverable error state.
    Error,
}

impl UseCaseState for ReactiveUseCaseState {
    fn is_error(&self) -> bool {
        self == &Self::Error
    }
}

/// Error representing the failure when sending a [ProtocolMessage].
///
/// Provided as goto Error for [UseCase]s when no other error can occur.
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct MessageSentFailed;

impl Display for MessageSentFailed {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Sending a ProtocolMessage through a MessageSender failed"
        )
    }
}

impl Error for MessageSentFailed {}

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
