//! Definitions for the routing protocol to interact with the underlay network.

use derive_more::derive::Display;
use derive_more::derive::From;
use std::num::NonZero;
use std::num::NonZeroUsize;

/// Represents a **connection** to an underlay neighbor.
///
/// Underlay Neighbors are nodes attached to the links of the KIRA node[^uln].
///
/// The [UnderlayNeighborId] is used to transparently inform the R²/KAD protocol
/// about a changed underlay environment.
/// Additionally they are used as "addresses" for sending protocol messages
/// over the underlay.
///
/// # Important
///
/// Since [UnderlayNeighborId] correspond to a **connection**,
/// it is possible that an underlay neighbor is addressable by more than one id.
/// It is the responsibility by the routing protocol to decide which connection
/// to use in case of multiple connections present.
///
/// There is at most one underlay neighbor reachable per [UnderlayNeighborId].
///
/// [^uln]: I.e., neighbors in the sense of [RFC8200][1] that
///     are *directly* reachable via link layer and the
///     Internet-layer or higher-layer tunnels.
///
/// [1]: <https://datatracker.ietf.org/doc/rfc8200/>
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Display, From)]
#[display("{_0:o}")]
pub struct UnderlayNeighborId(NonZeroUsize);

/// An underlay destination for [ProtocolMessages](crate::messaging::ProtocolMessage).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Display, From)]
pub enum UnderlayNeighborDestination {
    /// Broadcast to all underlay neighbors.
    ///
    /// This is primarily used when sending [HelloMessages](crate::messaging::HelloMessage)
    /// to  discover the underlay vicinity.
    ///
    /// Additionally Broadcast is set when receiving a [ProtocolMessages](crate::messaging::ProtocolMessage)
    /// from itself. See
    /// [BroadcastableUseCaseEvent::Message](crate::use_cases::BroadcastableUseCaseEvent::Message).
    Broadcast,
    /// Link to an underlay neighbor.
    UnderlayNeighbor(UnderlayNeighborId),
}

impl UnderlayNeighborDestination {
    /// Returns if the [destination](UnderlayNeighborDestination)
    /// is an [UnderlayNeighbor](UnderlayNeighborDestination::UnderlayNeighbor).
    pub const fn is_underlay_neighbor(&self) -> bool {
        matches!(self, Self::UnderlayNeighbor(_))
    }

    /// Returns if the [destination](UnderlayNeighborDestination)
    /// is [broadcast](UnderlayNeighborDestination::UnderlayNeighbor).
    pub const fn is_broadcast(&self) -> bool {
        matches!(self, Self::Broadcast)
    }
}

impl Default for UnderlayNeighborDestination {
    fn default() -> Self {
        Self::Broadcast
    }
}

impl From<Option<UnderlayNeighborId>> for UnderlayNeighborDestination {
    fn from(value: Option<UnderlayNeighborId>) -> Self {
        value.map(|ulnid| ulnid.into()).unwrap_or_default()
    }
}

impl From<usize> for UnderlayNeighborDestination {
    fn from(value: usize) -> Self {
        if value == 0 {
            Self::Broadcast
        } else {
            UnderlayNeighborId::from(NonZero::new(value).unwrap()).into()
        }
    }
}

/// The origin of [ProtocolMessages](crate::messaging::ProtocolMessage).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Display, From)]
pub enum UnderlayNeighborSource {
    /// The running protocol instance is the source.
    ///
    /// This is to support sending [ProtocolMessages](crate::messaging::ProtocolMessage)
    /// to ourselves.
    Local,
    /// Link to an underlay neighbor.
    UnderlayNeighbor(UnderlayNeighborId),
}

/// Updates of the currently present underlay neighbors connections.
#[derive(Debug, Clone, PartialEq)]
pub enum UnderlayNeighborUpdate {
    /// A new connection to an underlay neighbor was discovered.
    ///
    /// The connection may not provide connection to a new underlay neighbor
    /// since the node is already connected using a different connection.
    UnderlayNeighborUp(UnderlayNeighborId),
    /// A connection to an underlay neighbor was lost.
    UnderlayNeighborDown(UnderlayNeighborId),
}
