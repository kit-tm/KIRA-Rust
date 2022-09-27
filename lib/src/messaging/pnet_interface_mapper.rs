use std::net::SocketAddr;
use std::sync::Arc;

use pnet::datalink;
use pnet::ipnetwork::IpNetwork;
use tokio::sync::RwLock;

use crate::domain::NetworkInterface;
use crate::messaging::{AsyncInterfaceMapper, InterfaceMapper};

/// [PortMapper] and [AsyncPortMapper] implementation using [libpnet](https://docs.rs/pnet/)
/// to fetch network interface information and map the ip addresses to incoming ProtocolMessages.
///
/// Caches information and only refreshes information if an incoming message doesn't match
/// any interface.
#[derive(Debug, Clone, Default)]
pub struct PNetInterfaceMapper {
    interfaces: Arc<RwLock<Vec<datalink::NetworkInterface>>>,
}

impl PNetInterfaceMapper {
    /// Creates a new [PNetPortMapper] with empty cache.
    pub fn new() -> Self {
        Self {
            interfaces: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Refreshes the interface information cache by blocking the inner lock.
    pub fn blocking_refresh(&self) {
        let mut interfaces = self.interfaces.blocking_write();
        *interfaces = datalink::interfaces();

        log::trace!(target: "network_interfaces", "Found interfaces: {:?}", interfaces);
    }

    /// Refreshes the interface information cache.
    pub async fn refresh(&self) {
        let mut interfaces = self.interfaces.write().await;
        *interfaces = datalink::interfaces();

        log::trace!(target: "network_interfaces", "Found interfaces: {:?}", interfaces);
    }

    /// Find the [NetworkInterface] in the given iterator that matches the given [SocketAddr]
    /// and map it to its [Port] equivalent.
    fn find_in<'a, I: IntoIterator<Item = &'a datalink::NetworkInterface>>(
        addr: &SocketAddr,
        iter: I,
    ) -> Option<NetworkInterface> {
        let filter_network = |net: &IpNetwork| match (net, addr) {
            (IpNetwork::V4(network), SocketAddr::V4(addr)) => network.contains(*addr.ip()),
            (IpNetwork::V6(network), SocketAddr::V6(addr)) => network.contains(*addr.ip()),
            _ => false,
        };

        for interface in iter.into_iter() {
            if interface.ips.iter().any(filter_network) {
                return Some(NetworkInterface::from(interface));
            }
        }

        None
    }

    /// Get the [Port] to a given address out of the inner cache by blocking the lock.
    fn blocking_find(&self, addr: &SocketAddr) -> Option<NetworkInterface> {
        PNetInterfaceMapper::find_in(addr, self.interfaces.blocking_read().iter())
    }

    /// Get the [Port] to a given address out of the inner cache.
    async fn find(&self, addr: &SocketAddr) -> Option<NetworkInterface> {
        let interfaces = self.interfaces.read().await;
        PNetInterfaceMapper::find_in(addr, interfaces.iter())
    }
}

impl InterfaceMapper for PNetInterfaceMapper {
    fn get_interface(&self, input_addr: &SocketAddr) -> Option<NetworkInterface> {
        let blocking_find = self.blocking_find(input_addr);

        if blocking_find.is_none() {
            self.blocking_refresh();
            return self.blocking_find(input_addr);
        }

        blocking_find
    }
}

#[async_trait::async_trait]
impl AsyncInterfaceMapper for PNetInterfaceMapper {
    async fn get_interface(&self, input_addr: &SocketAddr) -> Option<NetworkInterface> {
        let find = self.find(input_addr).await;

        if find.is_none() {
            self.refresh().await;
            return self.find(input_addr).await;
        }

        find
    }
}
