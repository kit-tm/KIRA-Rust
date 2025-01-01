//! Definitions for the routing protocol to interact with the underlay network.

use derive_more::derive::Display;
use std::num::NonZeroUsize;

/// Represents a **connection** to an underlay neighbor.
///
/// Underlay Neighbors are nodes attached to the links of the KIRA node[^uln].
///
/// The underlay neighbor id is used to transparently inform the R²/KAD protocol
/// about a changed underlay environment.
/// Additionally they are used as "addresses" for sending protocol messages
/// over the underlay.
///
///
/// # Important
///
/// Since underlay neighbor ids correspond to a **connection**,
/// it is possible that an underlay neighbor is addressable by more than one id.
/// It is the responsibility by the routing protocol to decide which connection
/// to use in case of multiple connections present.
///
/// [^uln]: I.e., neighbors in the sense of [RFC8200][1] that
///     are *directly* reachable via link layer and the
///     Internet-layer or higher-layer tunnels.
/// [1]: https://datatracker.ietf.org/doc/rfc8200/
#[derive(Debug, Clone, PartialEq, Eq, Hash, Display)]
#[display("{_0:o}")]
pub enum UnderlayNeighborId {
    /// We are the underlink neighbor.
    ///
    /// This is useful if we want to send a message to our self.
    Loopback(),
    /// Link to different underlay neighbor.
    Other(NonZeroUsize),
}

// NOTE: maybe an event for **newly** discovered underlay neighbors could
//   be good this way we can immediately schedule a fast uln discovery

//   with being sure it's actually necessary.
/// Updates of the currently present underlay neighbors connections.
#[derive(Debug, Clone)]
pub enum UnderlayNeighborUpdate {
    /// A new connection to an underlay neighbor was discovered.
    ///
    /// The connection may not provide connection to a new underlay neighbor
    /// since the node is already connected using a different connection.
    UnderlayNeighborUp(UnderlayNeighborId),
    /// A connection to an underlay neighbor was lost.
    UnderlayNeighborDown(UnderlayNeighborId),
}
