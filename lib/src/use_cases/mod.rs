//! Implementations of the use cases.

use std::error::Error;
use std::fmt::{Display, Formatter};
use std::ops::Deref;

use crate::domain::{Contact, NetworkInterface};
use crate::hardware_events::HardwareEvent;
use crate::messaging::messages::ProtocolMessage;
use crate::messaging::{FindNodeReqData, Nonce};
use crate::messaging::dht::{DefaultLHTInput, FetchReqData, StoreReqData};

pub mod derive_fwd_table_entries;
pub mod explicit_path_management;
pub mod failure_handling;
pub mod forward_protocol_message;
pub mod handle_contact_update;
pub mod handle_overlay_discovery;
pub mod inject_messages;
pub mod overlay_neighborhood_discovery;
pub mod path_probing;
pub mod precompute_paths_and_path_ids;
pub mod random_overlay_discovery;
pub mod vicinity_discovery;
pub mod distributed_hash_table;
pub mod distributed_hash_table_injector;

/// Enumeration representing all events a [UseCase] can handle.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum UseCaseEvent {
    Message(ProtocolMessage, NetworkInterface),
    Timer(TimerId),
    Contact(ContactEvent),
    InjectMessage(Nonce, InjectionMessageData),
    Hardware(HardwareEvent),
    Shutdown,
}

/// Protocol message data to inject into the network.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum InjectionMessageData {
    FindNode(FindNodeReqData),
    Store(StoreReqData<DefaultLHTInput>),
    Fetch(FetchReqData),
}

/// Contact Events which can be handled by UseCases.
///
/// This is only a subset of [RoutingTableEvent](crate::domain::routing_table::observable_routing_table::RoutingTableEvent) as a UseCase should not react to
/// all kinds of [RoutingTableEvent](crate::domain::routing_table::observable_routing_table::RoutingTableEvent)s.
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

impl Display for TimerId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The state of a [UseCase] which is used to determine if a [UseCase] reached an unrecoverable state.
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

/// Returned by [EventHandler::handle_event].
///
/// Either means a [UseCaseEvent] was handled and should not delegated to the remaining use cases
/// or it was not handled and can be delegated.
#[derive(Debug)]
pub enum HandlingResult {
    /// A [UseCaseEvent] must not be delegated to the other [UseCase]s.
    Handled,
    /// A [UseCaseEvent] can be delegated to the other [UseCase]s.
    NotHandled,
}

/// An EventHandler handles events in some context and returns a value of type `Value` or an error
/// of type `Error`.
pub trait EventHandler {
    type Context;
    type Error;
    type Value;

    fn handle_event(
        &mut self,
        context: &Self::Context,
        event: UseCaseEvent,
    ) -> Result<Self::Value, Self::Error>;
}

/// A UseCase is an [EventHandler] which can be started in a given [UseCaseContext](crate::context::UseCaseContext), has a
/// [UseCaseState] and either returns a predefined Value or Error type.
pub trait UseCase: EventHandler {
    type State: UseCaseState + Sized;

    fn start(&mut self, context: &Self::Context) -> Result<(), Self::Error>;
    fn state(&self) -> &Self::State;
}
