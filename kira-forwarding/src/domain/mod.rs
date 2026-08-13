//! Domain Layer of the KIRA forwarding functionality.

#[doc(inline)]
pub use kira_r2kad::domain::{
    InterfaceId,
    NodeId,
    NodeIdSubnet,
    PathId,
    underlay::UnderlayNeighborId,
};

pub mod r2kad {
    //! R2Kad protocol events for the KIRA forwarding functionality
    #[doc(inline)]
    pub use kira_r2kad::domain::protocol_event::forwarding::*;
}

#[doc(inline)]
pub use kira_r2kad::domain::protocol_event::forwarding::{
    DecapsulationDestination,
    NodeIdEncapsulationEntry,
    NodeIdForwardingEntry,
    PathIdDecapsulationEntry,
    PathIdForwardingEntry,
};
