//! Domain Layer of the KIRA forwarding functionality.

#[doc(inline)]
pub use kira_lib::domain::{underlay::UnderlayNeighborId, NodeId, NodeIdSubnet, PathId};

pub mod r2kad {
    //! R2Kad protocol events for the KIRA forwarding functionality
    #[doc(inline)]
    pub use kira_lib::domain::protocol_event::forwarding::*;
}
