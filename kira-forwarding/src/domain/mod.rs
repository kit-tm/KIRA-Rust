//! Domain Layer of the KIRA forwarding functionality.

pub use kira_r2kad::domain::{
    DEFAULT_NODE_IP_PREFIX,
    DEFAULT_PATH_IP_PREFIX,
    EqualPrefixesError,
    InterfaceId,
    IpPrefixes,
    Ipv6Prefix16,
    Ipv6Prefix16Error,
    NodeId,
    NodeIdSubnet,
    PathId,
    UnderlayNeighborId,
};

pub mod r2kad {
    //! R2Kad protocol events for the KIRA forwarding functionality
    pub use kira_r2kad::domain::protocol_event::forwarding::*;
}

pub use kira_r2kad::domain::protocol_event::forwarding::{
    DecapsulationDestination,
    NodeIdEncapsulationEntry,
    NodeIdForwardingEntry,
    PathIdDecapsulationEntry,
    PathIdForwardingEntry,
};
