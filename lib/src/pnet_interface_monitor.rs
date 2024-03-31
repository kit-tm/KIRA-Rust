//! [InterfaceMapper], [AsyncInterfaceMapper] and [HardwareEventRegistry] implementation using [libpnet](https://docs.rs/pnet/)
//! to fetch network interface information and map the ip addresses to incoming [ProtocolMessages](crate::messaging::ProtocolMessage).

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;

use pnet::datalink;
use pnet::ipnetwork::{IpNetwork, Ipv6Network};
use tokio::sync::{Mutex, RwLock};

use crate::domain::NetworkInterface;
use crate::hardware_events::{HardwareEvent, HardwareEventHandler, HardwareEventRegistry};
use crate::messaging::{AsyncInterfaceMapper, InterfaceMapper};

/// [InterfaceMapper], [AsyncInterfaceMapper] and [HardwareEventRegistry] implementation using [libpnet](https://docs.rs/pnet/)
/// to fetch network interface information and map the ip addresses to incoming ProtocolMessages.
///
/// Also implements [HardwareEventRegistry] to handle hardware events.
///
/// Caches information and only refreshes information if an incoming message doesn't match
/// any interface.
#[derive(Default, Clone)]
pub struct PNetInterfaceMonitor {
    interfaces: Arc<RwLock<HashSet<datalink::NetworkInterface>>>,
    handlers: Arc<Mutex<Vec<Box<dyn HardwareEventHandler>>>>,
    excluded_interfaces: Arc<HashSet<u32>>,
}

impl PNetInterfaceMonitor {
    /// Creates a new [PNetInterfaceMonitor] with empty cache.
    pub fn new() -> Self {
        Self::with_excluded_interfaces(Default::default())
    }

    /// Creates a new [PNetInterfaceMonitor] which ignores some interfaces.
    pub fn with_excluded_interfaces(excluded_interfaces: HashSet<u32>) -> Self {
        Self {
            interfaces: Arc::new(RwLock::new(HashSet::new())),
            handlers: Arc::new(Mutex::new(Vec::new())),
            excluded_interfaces: Arc::new(excluded_interfaces),
        }
    }

    /// Returns filtered list of relevant interfaces present at the moment.
    fn new_interfaces(&self) -> HashSet<datalink::NetworkInterface> {
        fn is_link_local(ip: &&IpNetwork) -> bool {
            let IpNetwork::V6(ip) = ip else {
                return false;
            };

            let ll_network: Ipv6Network = "fe80::/64".parse().unwrap();
            ll_network.contains(ip.ip())
        }

        let interfaces = datalink::interfaces();

        // observation: all relevant interfaces have a MAC != 00:00:00:00:00:00, so maybe we can filter out those
        let interfaces = interfaces
            .into_iter()
            // filter out non-working interfaces
            .filter(|i| {
                i.is_up()
                    && !i.is_loopback()
                    && i.ips.iter().find(is_link_local).is_some()
                    && !i.name.contains("kira")
            })
            // filter out interfaces we want to ignore
            .filter(|i| !self.excluded_interfaces.contains(&i.index));

        HashSet::from_iter(interfaces)
    }

    /// Refreshes the interface information cache by blocking the inner lock.
    pub fn blocking_refresh(&self) {
        let mut interfaces = self.interfaces.blocking_write();

        let new_interfaces = self.new_interfaces();

        let (added, removed) = self.convert_to_events(interfaces.clone(), new_interfaces.clone());
        if let Some(added) = added {
            self.notify_all(added);
        }
        if let Some(removed) = removed {
            self.notify_all(removed);
        }

        *interfaces = new_interfaces;

        log::trace!(target: "network_interfaces", "Found interfaces: {:?}", interfaces);
    }

    /// Refreshes the interface information cache.
    ///
    /// **Still blocking**: As Pnet is not working asynchronously under the hood this may still
    /// block the current thread.
    pub async fn refresh(&self) {
        let mut interfaces = self.interfaces.write().await;

        let new_interfaces = self.new_interfaces();

        let (added, removed) = self.convert_to_events(interfaces.clone(), new_interfaces.clone());
        if let Some(added) = added {
            self.async_notify_all(added).await;
        }
        if let Some(removed) = removed {
            self.async_notify_all(removed).await;
        }

        *interfaces = new_interfaces;

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
        PNetInterfaceMonitor::find_in(addr, self.interfaces.blocking_read().iter())
    }

    /// Get the [Port] to a given address out of the inner cache.
    async fn find(&self, addr: &SocketAddr) -> Option<NetworkInterface> {
        let interfaces = self.interfaces.read().await;
        PNetInterfaceMonitor::find_in(addr, interfaces.iter())
    }

    fn convert_to_events(
        &self,
        mut before: HashSet<datalink::NetworkInterface>,
        after: HashSet<datalink::NetworkInterface>,
    ) -> (Option<HardwareEvent>, Option<HardwareEvent>) {
        let mut new_interfaces = HashSet::new();
        for interface_after in after {
            if !before.remove(&interface_after) {
                new_interfaces.insert(interface_after);
            }
        }
        // After this loop "before" contains only deleted interfaces (in 'before' but not in 'after')
        // And "new_interfaces" contains only new interfaces (not in 'before' but in 'after')
        let added_interfaces = if new_interfaces.is_empty() {
            None
        } else {
            Some(HardwareEvent::InterfacesUp(HashSet::from_iter(
                new_interfaces.iter().map(NetworkInterface::from),
            )))
        };
        let removed_interfaces = if new_interfaces.is_empty() {
            None
        } else {
            Some(HardwareEvent::InterfacesDown(HashSet::from_iter(
                before.iter().map(NetworkInterface::from),
            )))
        };
        (added_interfaces, removed_interfaces)
    }

    fn notify_all(&self, event: HardwareEvent) {
        let handlers = self.handlers.blocking_lock();
        for handler in handlers.iter() {
            handler.handle(event.clone());
        }
    }

    async fn async_notify_all(&self, event: HardwareEvent) {
        let handlers = self.handlers.lock().await;
        for handler in handlers.iter() {
            handler.handle(event.clone());
        }
    }
}

impl InterfaceMapper for PNetInterfaceMonitor {
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
impl AsyncInterfaceMapper for PNetInterfaceMonitor {
    async fn get_interface(&self, input_addr: &SocketAddr) -> Option<NetworkInterface> {
        let find = self.find(input_addr).await;

        if find.is_none() {
            self.refresh().await;
            return self.find(input_addr).await;
        }

        find
    }

    async fn get_available(&self) -> Vec<u32> {
        self.refresh().await;
        self.interfaces
            .read()
            .await
            .iter()
            .map(|i| i.index)
            .collect()
    }
}

impl HardwareEventRegistry for PNetInterfaceMonitor {
    fn register_handler<H: 'static + HardwareEventHandler>(&self, handler: H) {
        self.handlers.blocking_lock().push(Box::new(handler))
    }
}
