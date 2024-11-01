//! Events for interaction with the routing protocol R²/KAD.

use std::time::Instant;

use crate::domain::underlay::{UnderlayNeighborId, UnderlayNeighborUpdate};
use crate::messaging::{Nonce, ProtocolMessage};
use crate::use_cases::{ApiEvent, InjectionMessageData};

/// Events to externally control the [R2Kad](crate::R2Kad) protocol instance.
#[derive(Debug, Clone)]
pub enum Input {
    /// Received a new protocol message via the specified
    /// [underlay neighbor connection](UnderlayNeighborId).
    Message(ProtocolMessage, UnderlayNeighborId),
    /// Shutdown the routing protocol instance.
    Shutdown,
    /// Debug the routing protocol instance.
    Debug(DebugEvent),
    /// Some underlay connections where changed.
    UnderlayNeighborUpdate(UnderlayNeighborUpdate),
    /// Failed to send a requested [ProtocolMessage].
    SendFailed(ProtocolMessage, UnderlayNeighborId),
}

/// Events from the [R2Kad](crate::R2Kad) protocol to respond to.
#[derive(Debug, Clone)]
pub enum Output {
    /// Send a protocol message via the specified
    /// [underlay neighbor connection](UnderlayNeighborId).
    SendProtocolMessage(ProtocolMessage, UnderlayNeighborId),
    // TODO: move forwarding tables and make them accessible to the routing
    //   with this reserved event.
    /// Wish of the protocol to upgrade the forwarding table information.
    UpdateForwardingTables(()),
    //Timeout(Instant),
}

#[derive(Debug, Clone)]
pub enum DebugEvent {
    /// Interrogation events for protocol internal routing structures.
    Api(ApiEvent),
    /// Inject message with [Nonce] into the KIRA network from this protocol instance.
    InjectMessage(Option<Nonce>, InjectionMessageData),
}
