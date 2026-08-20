//! Definitions of the forwarding layer to interact with the [generic underlay information](UnderlayNeighborId)
//! provided by the [routing layer](../../kira_r2kad/index.html).

use std::net::Ipv6Addr;

use crate::domain::{
    InterfaceId,
    UnderlayNeighborId,
};

/// Provides implementations of the forwarding layer with information about the generic [UnderlayNeighborId].
#[trait_variant::make(UnderlayInformationProvider: Send)]
pub trait LocalUnderlayInformationProvider {
    /// Information provided by this provider.
    ///
    /// The information is typically made up of link-layer-specific information
    /// but also contains the link-local IPv6-address of the node.
    /// See [UnderlayNeighborInformation] for further info on the provided information.
    type Information;

    /// Error type returned if acquiring information for an [UnderlayNeighborId] failed.
    type Error;

    /// Tries to acquire [Information](Self::Information) for an [UnderlayNeighborId].
    ///
    /// If the [UnderlayNeighborId] is not known this function should return [None].
    async fn get_information(
        &mut self,
        ulnid: &UnderlayNeighborId,
    ) -> Result<Option<Self::Information>, Self::Error>;
}

/// Ethernet Address.
pub type EthAddr = [u8; 6];

/// All information known by KIRA about an underlay neighbor.
///
/// This information is used by the fast forwarding layer and the [io-part](../../kira_lib/io/index.html) of R²/KAD.
/// You can obtain this struct using [UnderlayObserverHandle::get_information](../../kira_lib/underlay/handle/struct.UnderlayObserverHandle.html#method.get_information)
#[derive(Debug, Clone)]
pub struct UnderlayNeighborInformation {
    /// Id of the interface under which the neighbor can be reached.
    pub interface_id: InterfaceId,
    /// Ethernet address used for sending messages to the neighbor from this interface.
    pub src_mac: EthAddr,
    /// Ethernet address used for broadcasting messages from this interface.
    pub broadcast_mac: EthAddr,
    /// link-local IPv6 address under which the neighbor can be reached.
    pub ll_ipv6: Ipv6Addr,
}
