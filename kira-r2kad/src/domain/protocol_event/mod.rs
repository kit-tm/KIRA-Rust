//! Events for interaction with the routing protocol R²/KAD.

pub mod forwarding;

use std::net::Ipv6Addr;

#[doc(inline)]
pub use forwarding::{
    ForwardingTablesUpdate,
    NodeIdTableUpdate,
    PathIdTableUpdate,
};

use crate::{
    domain::{
        NodeId,
        protocol_message::{
            Nonce,
            ProtocolMessage,
        },
        underlay::{
            UnderlayNeighborDestination,
            UnderlayNeighborId,
            UnderlayNeighborUpdate,
        },
    },
    use_cases::{
        ApiEvent,
        InjectionMessageData,
        UseCaseEvent,
    },
};

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
    /// The forwarding tier dropped a data packet because its PathIP is unknown.
    DroppedPacket(DroppedPacket),
    ///// Failed to send a requested [ProtocolMessage].
    //SendFailed(ProtocolMessage, UnderlayNeighborId),
}

/// A data packet the forwarding tier of this node dropped because it has no entry for its
/// PathIP, e.g., as the link to the next hop failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DroppedPacket {
    /// The node that encapsulated the packet with the PathIP, i.e., its last overlay hop,
    /// which used an obsolete path.
    pub encapsulator: NodeId,
    /// The unknown PathIP, i.e., the PathId of the remaining path at this node (first 112
    /// bits, see [IpPrefixes::path_ip](crate::domain::IpPrefixes::path_ip)).
    pub path_ip: Ipv6Addr,
    /// Destination of the encapsulated packet, if it is a NodeIP.
    pub destination: Option<NodeId>,
}

/// Events from the [R2Kad](crate::R2Kad) protocol to respond to.
#[derive(Debug, Clone)]
pub enum Output {
    /// Send a protocol message via the specified
    /// [underlay neighbor connection](UnderlayNeighborId).
    SendProtocolMessage(ProtocolMessage, UnderlayNeighborDestination),
    /// Request to update information in the forwarding functionality.
    UpdateForwardingTables(ForwardingTablesUpdate),
}

/// Events for inspecting the internals of the protocol instance.
#[derive(Debug, Clone)]
pub enum DebugEvent {
    /// Interrogation events for protocol internal routing structures.
    Api(ApiEvent),
    /// Inject message with [Nonce] into the KIRA network from this protocol instance.
    InjectMessage(Option<Nonce>, InjectionMessageData),
}

impl From<ApiEvent> for DebugEvent {
    fn from(value: ApiEvent) -> Self {
        Self::Api(value)
    }
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
            Input::DroppedPacket(dropped) => Self::DroppedPacket(dropped),
        }
    }
}
