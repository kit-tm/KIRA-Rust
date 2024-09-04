//! Additional context for the eBPF ForwardingTable implementation not covered
//! by the [ForwardingTables](crate::forwarding::ForwardingTables) trait signature

use thiserror::Error;

use crate::domain::NodeId;
use kira_bpf_common::domain::kira::forwarding::maps::NextHop;

/// CRUD access like interface for managing [NextHop] entries.
///
/// The implementer has to **create** the [NextHop] based on the provided information:
///
/// - [NodeId]
/// - name of a [NetworkInterface]
///
/// This will create an association between a (neighboring) [NodeId]
/// and a corresponding [NextHop] for users.
///
/// # Caveats
///
/// It is currently not supported to address a neighboring [NodeId]
/// as a [NextHop] via different interfaces. Thereby the [get](NextHopCache::get)
/// method does not provide a way to specify a [NetworkInterface].
pub trait NextHopContext {
    type Error;

    /// Constructs a [NextHop] entry representing the given `next_hop`.
    ///
    /// # Errors
    ///
    /// This function will return an error if a [NextHop] entry for the
    /// [NodeId] already  does exist.
    fn create(&mut self, next_hop: &NodeId, interface: u32) -> Result<&NextHop, Self::Error>;

    fn get(&self, next_hop: &NodeId) -> Result<Option<&NextHop>, Self::Error>;

    /// Updates a [NextHop] entry representing the given `next_hop`.
    ///
    /// # Errors
    ///
    /// This function will return an error if no [NextHop] entry for the
    /// [NodeId] does exist.
    fn update(&mut self, next_hop: &NodeId, interface: u32) -> Result<&NextHop, Self::Error>;

    /// Constructs or updates a [NextHop] entry representing the given `next_hop`.
    ///
    /// # Errors
    ///
    /// This function will not return an error if a [NextHop] does exist or not.
    fn create_or_update(
        &mut self,
        next_hop: &NodeId,
        interface: u32,
    ) -> Result<&NextHop, Self::Error>;

    /// Removes a [NextHop] entry.
    ///
    /// # Errors
    ///
    /// This function will not return an error if no [NextHop] entry does exist.
    fn remove(&mut self, next_hop: &NodeId) -> Result<Option<NextHop>, Self::Error>;
}

#[derive(Debug, Error)]
pub enum NextHopContextError {
    #[error("interface has not MAC")]
    InterfaceWithoutMac(),
    #[error("no interface with index found")]
    UnknownInterfaceIndex(),
}

pub enum PathIdEntryContext {
    NextHop(NextHop),
    EndOfPath(), // additional information needed if end of path
}

#[cfg(feature = "pnet")]
pub mod pnet_next_hop_context {
    //! Default implementation of the [NextHopContext] using [pnet].
    use super::*;

    use std::collections::{
        hash_map::Entry::{Occupied, Vacant},
        HashMap,
    };

    use pnet::util::MacAddr;

    use crate::domain::NodeId;
    use kira_bpf_common::domain::kira::forwarding::maps::NextHop;
    #[derive(Default)]
    pub struct PnetNextHopContext {
        src_mac_cache: HashMap<String, MacAddr>,
        next_hop_cache: HashMap<NodeId, NextHop>,
    }

    impl NextHopContext for PnetNextHopContext {
        type Error = NextHopContextError;

        fn create(&mut self, next_hop: &NodeId, interface: u32) -> Result<&NextHop, Self::Error> {
            todo!()
        }

        fn get(&self, next_hop: &NodeId) -> Result<Option<&NextHop>, Self::Error> {
            Ok(self.next_hop_cache.get(next_hop))
        }

        fn update(&mut self, next_hop: &NodeId, interface: u32) -> Result<&NextHop, Self::Error> {
            todo!()
        }

        fn create_or_update(
            &mut self,
            next_hop: &NodeId,
            interface: u32,
        ) -> Result<&NextHop, Self::Error> {
            // using entry instead of insert_with to be able to short circuit
            let src_mac_entry = self.src_mac_cache.entry(interface.to_string());
            let src_mac = match src_mac_entry {
                Occupied(occupied_entry) => occupied_entry.into_mut(),
                Vacant(vacant_entry) => {
                    let src_mac = pnet::datalink::interfaces()
                        .iter()
                        .find(|iface| iface.index == interface)
                        .map(|iface| iface.mac.ok_or(NextHopContextError::InterfaceWithoutMac()))
                        .ok_or(NextHopContextError::UnknownInterfaceIndex())??;

                    vacant_entry.insert(src_mac)
                }
            };

            let next_hop_entry = NextHop {
                src_mac: src_mac.octets(),
                // FIXME get proper destination mac: NID -[IpCache]-> LLIp --> ip neigh get
                dst_mac: [255u8; 6],
                interface_idx: interface,
            };

            let next_hop = self
                .next_hop_cache
                .entry(next_hop.clone())
                .and_modify(|e| *e = next_hop_entry)
                .or_insert(next_hop_entry);
            Ok(next_hop)
        }

        fn remove(&mut self, next_hop: &NodeId) -> Result<Option<NextHop>, Self::Error> {
            Ok(self.next_hop_cache.remove(next_hop))
        }
    }
}

#[cfg(feature = "pnet")]
pub use pnet_next_hop_context::PnetNextHopContext;
