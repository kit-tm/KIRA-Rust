//! Definitions of the forwarding layer to interact with the [generic underlay information](UnderlayNeighborId)
//! provided by the [routing layer](kira_lib::R2Kad).

use std::net::Ipv6Addr;

use crate::domain::{InterfaceId, UnderlayNeighborId};

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

/// Ethernet Address.
pub type EthAddr = [u8; 6];

#[derive(Debug, Clone)]
/// All information known by KIRA about an underlay neighbor.
///
/// This information is used by the fast forwarding layer and the [io-part](crate::io) of R²/KAD.
/// You can obtain this struct using [UnderlayObserverHandle::get_information](crate::underlay::UnderlayObserverHandle::get_information)
pub struct UnderlayNeighborInformation {
    /// Id of the [Interface] under which the neighbor can be reached.
    pub interface_id: InterfaceId,
    /// Ethernet address used for sending messages to the neighbor from this [Interface].
    pub src_mac: EthAddr,
    /// Ethernet address used for broadcasting messages from this [Interface].
    pub broadcast_mac: EthAddr,
    /// link-local IPv6 address under which the neighbor can be reached.
    pub ll_ipv6: Ipv6Addr,
}
