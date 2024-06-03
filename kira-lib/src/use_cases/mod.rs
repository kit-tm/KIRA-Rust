//! Implementations of the use cases.

use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::ops::Deref;
use tokio::sync::mpsc; // use tokio::sync::oneshot;

use crate::domain::{Contact, NetworkInterface, NodeId, StateSeqNr};
use crate::hardware_events::HardwareEvent;
use crate::messaging::dht::{DefaultLHTInput, DefaultLHTOutput, FetchErr};
use crate::messaging::messages::ProtocolMessage;
use crate::messaging::{FindNodeReqData, Nonce};
use crate::use_cases::inject_messages::InjectionResult;

pub mod derive_fwd_table_entries;
pub mod distributed_hash_table;
pub mod distributed_hash_table_injector;
pub mod explicit_path_management;
pub mod failure_handling;
pub mod forward_protocol_message;
pub mod handle_api;
pub mod handle_contact_update;
pub mod handle_overlay_discovery;
pub mod inject_messages;
pub mod overlay_neighborhood_discovery;
pub mod path_probing;
pub mod precompute_paths_and_path_ids;
pub mod random_overlay_discovery;
pub mod vicinity_discovery;

/// Callback used to message back an [InjectionResult] to an injector.
///
/// This callback channel is usually used if a [ReqRspMessage](super::messaging::ReqRspMessage) is injected
/// into the network return the respective response message.
pub type OneshotInjectMessageCallback = mpsc::UnboundedSender<InjectionResult>; // todo change back to oneshot after we figured out how to eliminate the need of deriving clone

/// Enumeration representing all events a [UseCase] can handle.
#[derive(Debug, Clone, PartialEq)]
pub enum UseCaseEvent {
    Message(ProtocolMessage, NetworkInterface),
    Timer(TimerId),
    Contact(ContactEvent),
    ResyncNode(NodeId, StateSeqNr),
    InjectMessage(Option<Nonce>, InjectionMessageData),
    Hardware(HardwareEvent),
    API(ApiEvent),
    Shutdown,
}

/// Protocol message data to inject into the network.
#[derive(Debug, Clone)]
pub enum InjectionMessageData {
    FindNode(FindNodeReqData),
    Store(
        StoreInjectData<DefaultLHTInput>,
        OneshotInjectMessageCallback,
    ),
    Fetch(FetchInjectData, OneshotInjectMessageCallback),
}

impl PartialEq for InjectionMessageData {
    fn eq(&self, other: &Self) -> bool {
        match self {
            Self::FindNode(data) => {
                if let Self::FindNode(other_data) = other {
                    return data == other_data;
                }
            }
            Self::Store(data, sender) => {
                if let Self::Store(other_data, other_sender) = other {
                    return data == other_data && sender.same_channel(other_sender);
                }
            }
            Self::Fetch(data, sender) => {
                if let Self::Fetch(other_data, other_sender) = other {
                    return data == other_data && sender.same_channel(other_sender);
                }
            }
        }

        false
    }
}

#[derive(Debug, Clone)]
pub enum ApiEvent {
    LocalHashTable(mpsc::UnboundedSender<Result<Vec<(NodeId, DefaultLHTOutput)>, FetchErr>>),
    PNTable(mpsc::UnboundedSender<String>),
    RoutingTable(mpsc::UnboundedSender<String>),
    VicinityGraph(mpsc::UnboundedSender<String>),
}

impl PartialEq for ApiEvent {
    fn eq(&self, other: &Self) -> bool {
        match self {
            ApiEvent::LocalHashTable(sender) => {
                if let ApiEvent::LocalHashTable(other_sender) = other {
                    return sender.same_channel(other_sender);
                }
            }
            ApiEvent::PNTable(sender) => {
                if let ApiEvent::PNTable(other_sender) = other {
                    return sender.same_channel(other_sender);
                }
            }
            ApiEvent::RoutingTable(sender) => {
                if let ApiEvent::RoutingTable(other_sender) = other {
                    return sender.same_channel(other_sender);
                }
            }
            ApiEvent::VicinityGraph(sender) => {
                if let ApiEvent::VicinityGraph(other_sender) = other {
                    return sender.same_channel(other_sender);
                }
            }
        }

        false
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct StoreInjectData<D: Debug> {
    pub handle: NodeId,
    pub data: D,
    pub restore: bool,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FetchInjectData {
    pub handle: NodeId,
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
    BucketUpdated(usize),
    NewBucket(usize),
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

#[cfg(test)]
mod tests {
    use crate::messaging::ProtocolMessage;
    use crate::use_cases::UseCaseEvent;
    use std::sync::mpsc::Receiver;

    pub fn wait_for_message(recv: Receiver<UseCaseEvent>) -> ProtocolMessage {
        loop {
            let response = recv.recv();
            assert!(response.is_ok(), "Error receiving result: {:?}", response);

            if let UseCaseEvent::Message(message, ..) = response.unwrap() {
                break message;
            }
        }
    }
}

