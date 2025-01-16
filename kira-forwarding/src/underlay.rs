//! Definitions of the forwarding layer to interact with the [generic underlay information](UnderlayNeighborId)
//! provided by the [routing layer](kira_lib::R2Kad).

use std::net::Ipv6Addr;

#[doc(inline)]
pub use kira_lib::domain::underlay::UnderlayNeighborId;

/// Provides implementations of the forwarding layer with information about the generic [UnderlayNeighborId].
pub trait UnderlayInformationProvider {
    /// Information provided by this provider.
    ///
    /// The information is typically made up of [link-layer-specific information](MacLayerInformation)
    /// but also contains the link-local IPv6-address of the node. See
    /// [UnderlayInformation] for further info on the provided information.
    type Information;

    /// Error type returned if acquiring [Information] for an [UnderlayNeighborId] failed.
    type Error;

    /// Tries to acquire [Information] for an [UnderlayNeighborId].
    ///
    /// If the [UnderlayNeighborId] is not know this function should error.
    fn get_information(&self, ulnid: &UnderlayNeighborId)
        -> Result<Self::Information, Self::Error>;
}

pub struct MacLayerInformation {
    pub src: [u8; 14],
    pub dst: [u8; 14],
    pub interface_index: u16,
}

pub struct UnderlayInformation {
    pub mac: MacLayerInformation,
    /// The link-local IPv6 on which the neighbor can be reached.
    ///
    /// # Note
    ///
    /// This is *not* the NodeId-IPv6 (IPv6 with prefix `fc00::/16`).
    pub ll_ipv6: Ipv6Addr,
}
