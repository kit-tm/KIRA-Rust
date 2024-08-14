use std::collections::HashMap;
use std::sync::{Arc, Weak};

use kira_bpf_common::aya::programs::{xdp::XdpLink, Xdp, XdpFlags};
use kira_bpf_common::aya::BpfError;

use crate::domain::NodeId;

pub struct XdpHandle {
    xdp: Xdp,
    attached_links: HashMap<String, Weak<XdpLink>>,
    attached_neighbors: HashMap<NodeId, Arc<XdpLink>>,
}

impl XdpHandle {
    pub fn new(xdp: Xdp) -> Self {
        Self {
            xdp,
            attached_links: Default::default(),
            attached_neighbors: Default::default(),
        }
    }

    pub fn attach(&mut self, physical_neighbor: NodeId, iface: String) -> Result<(), BpfError> {
        // if we already are attached to the interface
        // we don't need to attach again
        let attached_link = if let Some(attached_link) = self
            .attached_links
            .get(&iface)
            // upgrade can fail if neighbors using the interface where detached already
            .and_then(|weak_attached_link| weak_attached_link.upgrade())
        {
            attached_link
        } else {
            // TODO support and try other flags and impact
            let link_id = self.xdp.attach(&iface, XdpFlags::default())?;
            log::debug!("Attached to interface: {iface}");

            let attached_link = self.xdp.take_link(link_id)?;
            let attached_link = Arc::new(attached_link);

            // save link so other neighbors can attach over the same interface
            // but don't prohibit detach by caching them => downgrade
            self.attached_links
                .insert(iface, Arc::downgrade(&attached_link.clone()));

            attached_link
        };

        self.attached_neighbors
            .insert(physical_neighbor, attached_link);

        Ok(())
    }

    pub fn detach(&mut self, physical_neighbor: &NodeId) {
        // reference in attached_links is weak so won't prevent Drop
        let _ = self.attached_neighbors.remove(physical_neighbor);

        log::debug!("Detached physical neighbor: {physical_neighbor}");
    }
}
