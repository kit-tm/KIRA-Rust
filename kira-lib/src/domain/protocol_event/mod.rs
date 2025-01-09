//! Events for interaction with the routing protocol R²/KAD.

use crate::domain::underlay::{UnderlayNeighborId, UnderlayNeighborUpdate};
use crate::domain::UnderlayNeighborDestination;
use crate::messaging::{Nonce, ProtocolMessage};
use crate::use_cases::{ApiEvent, InjectionMessageData, UseCaseEvent};

pub mod forwarding;
use forwarding::ForwardingTablesUpdate;

/// Events to externally control the [R2Kad](crate::R2Kad) protocol instance.
#[derive(Debug, Clone)]
pub enum Input {
    /// Received a new protocol message via the specified
    /// [underlay neighbor connection](UnderlayNeighborId).
    Message(ProtocolMessage, UnderlayNeighborId),
    /// Debug the routing protocol instance.
    Debug(DebugEvent),
    /// Some underlay connections where changed.
    UnderlayUpdate(UnderlayNeighborUpdate),
    ///// Failed to send a requested [ProtocolMessage].
    //SendFailed(ProtocolMessage, UnderlayNeighborId),
}

/// Events from the [R2Kad](crate::R2Kad) protocol to respond to.
#[derive(Debug, Clone)]
pub enum Output {
    /// Send a protocol message via the specified
    /// [underlay neighbor connection](UnderlayNeighborId).
    SendProtocolMessage(ProtocolMessage, UnderlayNeighborDestination),
    /// Send a protocol message to all underlay neighbors.
    ///
    /// # Important
    ///
    /// The message must also be forwarded to neighbors who
    /// haven't exchanged any messages or have joined the network.
    BroadCastProtocolMessage(ProtocolMessage),
    /// Request to update information in the forwarding functionality.
    UpdateForwardingTables(ForwardingTablesUpdate),
    //Timeout(Instant),
}

/// Events for inspecting the internals of the protocol instance.
#[derive(Debug, Clone)]
pub enum DebugEvent {
    /// Interrogation events for protocol internal routing structures.
    Api(ApiEvent),
    /// Inject message with [Nonce] into the KIRA network from this protocol instance.
    InjectMessage(Option<Nonce>, InjectionMessageData),
}

impl From<Input> for UseCaseEvent {
    fn from(value: Input) -> Self {
        match value {
            Input::Message(pm, ulnid) => Self::Message(pm, ulnid.into()),
            Input::Debug(DebugEvent::Api(api_event)) => Self::API(api_event),
            Input::Debug(DebugEvent::InjectMessage(nonce, data)) => {
                Self::InjectMessage(nonce, data)
            }
            Input::UnderlayUpdate(up) => Self::UnderlayUpdate(up),
        }
    }
}
