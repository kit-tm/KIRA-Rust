//! Definitions of the forwarding layer to interact with the [generic underlay information](UnderlayNeighborId)
//! provided by the [routing layer](kira_lib::R2Kad).

use crate::domain::UnderlayNeighborId;

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
