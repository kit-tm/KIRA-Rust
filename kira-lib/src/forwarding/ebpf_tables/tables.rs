use aya_log::BpfLogger;
use derive_more::From;
use std::path::Path;
use thiserror::Error;

use kira_bpf_common::ebpf_utils;
use kira_bpf_common::{
    aya::{maps::MapError, programs::ProgramError, BpfError},
    domain::kira::forwarding::maps::NextHop,
    ebpf_utils::EbpfUtilError,
};

use crate::forwarding::ebpf;
use crate::{
    domain::{NodeId, NodeIdSubnet, PathId, SIZE},
    forwarding::{
        ebpf_tables::domain::{
            context::{NextHopContext, NextHopContextError, PathIdEntryContext},
            entry_strategy::{EbpfEntryStrategy, NodeIdEntryStrategy, PathIdEntryStrategy},
            xdp::XdpAttachType,
            xdp::XdpHandle,
        },
        ForwardingTables, NodeIdEncapsulationEntry, NodeIdEntry, NodeIdForwardingEntry,
        NodeIdTable, PathIdDecapsulationEntry, PathIdEntry, PathIdForwardingEntry, PathIdTable,
    },
};

#[derive(Debug, Error, From)]
pub enum EbpfFwdTablesError {
    #[error(transparent)]
    ProgramError(ProgramError),
    #[error(transparent)]
    EbpfError(BpfError),
    #[error(transparent)]
    EbpfUtilError(EbpfUtilError),
    #[error(transparent)]
    NextHopContextError(NextHopContextError),
    #[error(transparent)]
    MapError(MapError),
}

pub struct EbpfFwdTables<H, E> {
    root_id: NodeId,
    pub next_hop_context: H,
    entry_strategy: E,
    xdp: XdpHandle,
    attach_type: XdpAttachType,
}

impl<H, E> EbpfFwdTables<H, E> {
    fn attach(
        &mut self,
        physical_neighbor: NodeId,
        iface: String,
    ) -> Result<(), EbpfFwdTablesError> {
        self.xdp
            .attach(physical_neighbor, iface, self.attach_type)?;
        Ok(())
    }

    fn detach(&mut self, physical_neighbor: &NodeId) {
        self.xdp.detach(physical_neighbor);
    }

    // Detaching automatically if if goes down
}
impl<H, E> EbpfFwdTables<H, E>
where
    E: EbpfEntryStrategy,
    EbpfFwdTablesError: From<<E as EbpfEntryStrategy>::Error>,
{
    pub fn with_attach_type<P>(
        path: P,
        root_id: NodeId,
        next_hop_context: H,
        attach_type: XdpAttachType,
    ) -> Result<Self, EbpfFwdTablesError>
    where
        P: AsRef<Path>,
    {
        let mut bpf = ebpf_utils::load_bpf_from_file(&root_id.clone().into(), path)?;
        if let Err(e) = BpfLogger::init(&mut bpf) {
            log::warn!("Failed to initialize BpfLogger: {e}");
        }

        // TODO bump_memlock_rlimit
        // TODO setup default route including MTU

        let entry_strategy = E::with_bpf(&mut bpf)?;

        // HACK obtain actual Xdp program
        let xdp = ebpf_utils::load_xdp(&mut bpf)?;
        ebpf_utils::pin_xdp(xdp, "kira-forwarding", "xdp_forwarding")?;
        let mut xdp = ebpf_utils::load_pinned_xdp("kira-forwarding", "xdp_forwarding")?;

        // setup route and forwarding for local packets
        ebpf::create_nid_default_route().unwrap();
        xdp.attach("lo", XdpAttachType::SkbMode.into())?;

        let xdp = XdpHandle::new(xdp);
        let table = Self {
            root_id,
            next_hop_context,
            entry_strategy,
            xdp,
            attach_type,
        };
        Ok(table)
    }

    pub fn load_from_file<P>(
        path: P,
        root_id: NodeId,
        next_hop_context: H,
    ) -> Result<Self, EbpfFwdTablesError>
    where
        P: AsRef<Path>,
    {
        Self::with_attach_type(path, root_id, next_hop_context, Default::default())
    }
}

impl<H, E> Drop for EbpfFwdTables<H, E> {
    fn drop(&mut self) {
        // unpin on drop
        ebpf_utils::unpin_xdp("kira-forwarding", "xdp_forwarding")
            .expect("Unpinning should be successfull");

        ebpf::delete_nid_default_route()
            .expect("default fc00::/128 route should have been setup at initialization")
    }
}

impl<H, E> EbpfFwdTables<H, E>
where
    H: NextHopContext,
    EbpfFwdTablesError: From<H::Error>,
{
    // always create_or_update for NextHopContext
    // since other entry could already be using next_hop
    // update could also switch to virgin next_hop

    fn next_hop_with_node_id_entry(
        &mut self,
        entry: &NodeIdEntry,
    ) -> Result<NextHop, EbpfFwdTablesError> {
        let next_hop = match entry {
            NodeIdEntry::Forward(NodeIdForwardingEntry {
                next_hop,
                out_interface,
                ..
            })
            | NodeIdEntry::Encapsulate(NodeIdEncapsulationEntry {
                next_hop,
                out_interface,
                ..
            }) => {
                self.attach(next_hop.clone(), out_interface.name.clone())?;
                self.next_hop_context
                    .create_or_update(next_hop, out_interface.index)?
            }
        };

        Ok(*next_hop)
    }
}

impl<H, E> NodeIdTable for EbpfFwdTables<H, E>
where
    H: NextHopContext,
    E: NodeIdEntryStrategy<Context = NextHop>,
    EbpfFwdTablesError: From<H::Error>,
    EbpfFwdTablesError: From<E::Error>,
{
    type Error = EbpfFwdTablesError;

    fn create(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error> {
        let next_hop = self.next_hop_with_node_id_entry(&entry)?;
        self.entry_strategy.create(entry, next_hop)?;

        Ok(())
    }

    fn update(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error> {
        let next_hop = self.next_hop_with_node_id_entry(&entry)?;
        self.entry_strategy.update(entry, next_hop)?;

        Ok(())
    }

    fn create_or_update(&mut self, entry: NodeIdEntry) -> Result<(), Self::Error> {
        let next_hop = self.next_hop_with_node_id_entry(&entry)?;
        self.entry_strategy.create_or_update(entry, next_hop)?;

        Ok(())
    }

    fn remove(&mut self, node_id: &NodeIdSubnet) -> Result<Option<NodeIdEntry>, Self::Error> {
        // only remove on real entry
        if node_id.prefix_length == SIZE || node_id.prefix_length == 0 {
            let _ = self.next_hop_context.remove(&node_id.node_id)?;
            self.detach(&node_id.node_id);
        }

        let entry = self.entry_strategy.remove(node_id)?;
        Ok(entry)
    }
}

impl<H, E> EbpfFwdTables<H, E>
where
    H: NextHopContext,
    EbpfFwdTablesError: From<H::Error>,
{
    fn next_hop_with_path_id_entry(
        &self,
        entry: &PathIdEntry,
    ) -> Result<PathIdEntryContext, EbpfFwdTablesError> {
        let next_hop = match entry {
            PathIdEntry::Forward(PathIdForwardingEntry { next_hop, .. })
            | PathIdEntry::Decapsulate(PathIdDecapsulationEntry {
                local_id: next_hop, ..
            }) => {
                if next_hop == &self.root_id {
                    PathIdEntryContext::EndOfPath()
                } else {
                    let next_hop = self.next_hop_context.get(next_hop)?.expect(
                        "Physical neighbor for PathId should be setup for forwarding first",
                    );

                    PathIdEntryContext::NextHop(*next_hop)
                }
            }
        };

        Ok(next_hop)
    }
}

impl<H, E> PathIdTable for EbpfFwdTables<H, E>
where
    H: NextHopContext,
    E: PathIdEntryStrategy<Context = PathIdEntryContext>,
    EbpfFwdTablesError: From<H::Error>,
    EbpfFwdTablesError: From<E::Error>,
{
    type Error = EbpfFwdTablesError;

    fn create(&mut self, entry: PathIdEntry) -> Result<(), Self::Error> {
        let next_hop = self.next_hop_with_path_id_entry(&entry)?;
        self.entry_strategy.create(entry, next_hop)?;
        Ok(())
    }

    fn update(&mut self, entry: PathIdEntry) -> Result<(), Self::Error> {
        let next_hop = self.next_hop_with_path_id_entry(&entry)?;
        self.entry_strategy.update(entry, next_hop)?;
        Ok(())
    }

    fn create_or_update(&mut self, entry: PathIdEntry) -> Result<(), Self::Error> {
        let next_hop = self.next_hop_with_path_id_entry(&entry)?;
        self.entry_strategy.create_or_update(entry, next_hop)?;
        Ok(())
    }

    fn remove(&mut self, path_id: &PathId) -> Result<Option<PathIdEntry>, Self::Error> {
        // no need to remove NextHopContext entry since this is done by removing
        // the NodeIdEntry
        let entry = self.entry_strategy.remove(path_id)?;
        Ok(entry)
    }
}

impl<H, E> ForwardingTables for EbpfFwdTables<H, E>
where
    H: NextHopContext,
    E: PathIdEntryStrategy<Context = PathIdEntryContext> + NodeIdEntryStrategy<Context = NextHop>,
    EbpfFwdTablesError: From<H::Error>,
    EbpfFwdTablesError: From<<E as PathIdEntryStrategy>::Error>,
    EbpfFwdTablesError: From<<E as NodeIdEntryStrategy>::Error>,
{
}
