#[cfg(feature = "ebpf-log")]
use aya_log::EbpfLogger;

use derive_more::From;
use std::marker::PhantomData;
use std::path::Path;
use thiserror::Error;

use kira_bpf_common::ebpf_utils;
use kira_bpf_common::{
    aya::{maps::MapError, programs::ProgramError, Ebpf, EbpfError},
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
    EbpfError(EbpfError),
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

/// Builder pattern for construction [EbpfFwdTables].
pub struct EbpfFwdTablesBuilder<H, E> {
    root_id: NodeId,
    next_hop_context: H,
    entry_strategy: PhantomData<E>, // for builder() method
    bpf: Option<Ebpf>,
    attach_type: XdpAttachType,

    #[cfg(feature = "ebpf-log")]
    init_log: bool,
}

impl<H: Default, E> EbpfFwdTables<H, E> {
    /// Create a builder.
    ///
    /// This builder has a default [NodeId] of [zero](NodeId::zero).
    /// Remember to change it to your actual [NodeId] with
    /// [EbpfFwdTablesBuilder::root_id]
    pub fn builder() -> EbpfFwdTablesBuilder<H, E> {
        EbpfFwdTablesBuilder::new(H::default(), NodeId::zero())
    }
}

impl<H, E> EbpfFwdTablesBuilder<H, E> {
    pub fn new(next_hop_context: H, root_id: NodeId) -> Self {
        Self {
            root_id,
            next_hop_context,
            entry_strategy: Default::default(),
            bpf: Default::default(),
            attach_type: Default::default(),

            #[cfg(feature = "ebpf-log")]
            init_log: Default::default(),
        }
    }

    /// Set [ROOT_ID](kira_bpf_common::domain::kira::forwarding::maps::ROOT_ID) for builder.
    pub fn root_id(mut self, root_id: NodeId) -> Self {
        self.root_id = root_id;
        self
    }

    pub fn attach_type(mut self, attach_type: XdpAttachType) -> Self {
        self.attach_type = attach_type;
        self
    }

    /// Use [Ebpf] for constructing the [EbpfFwdTables].
    ///
    /// # Safety
    ///
    /// Make sure the set the
    /// [ROOT_ID](kira_bpf_common::domain::kira::forwarding::maps::ROOT_ID)
    /// yourself using [EbpfLoader::set_global](kira_bpf_common::aya::EbpfLoader::set_global).
    pub unsafe fn bpf(mut self, bpf: Ebpf) -> Self {
        self.bpf = Some(bpf);
        self
    }

    /// Loads [Ebpf] from the given [Path].
    ///
    /// This function also sets the
    /// [ROOT_ID](kira_bpf_common::domain::kira::forwarding::maps::ROOT_ID)
    /// Make sure you set the right root-id using [Self::root_id]
    pub fn bpf_from_file<P>(mut self, path: P) -> Result<Self, EbpfFwdTablesError>
    where
        P: AsRef<Path>,
    {
        // FIXME call this on final build instruction
        let bpf = ebpf_utils::load_bpf_from_file(&self.root_id.clone().into(), path)?;
        self.bpf = Some(bpf);
        Ok(self)
    }

    /// Initialize the [EbpfLogger].
    ///
    /// Requires that the builder is inside a tokio runtime.
    ///
    /// # Error
    ///
    /// On error the current builder instance is returned,
    /// because failing to initialize the log
    /// is not a fatal error on building.
    #[cfg(feature = "ebpf-log")]
    pub fn init_log(mut self) -> Self {
        self.init_log = true;
        self
    }

    pub fn build_with_strategy(
        mut self,
        strategy: E,
    ) -> Result<EbpfFwdTables<H, E>, EbpfFwdTablesError> {
        #[cfg(feature = "ebpf-log")]
        if self.init_log {
            let bpf = self
                .bpf
                .as_mut()
                .expect("Ebpf should have been set with `Self::bpf`");
            if EbpfLogger::init(bpf).is_err() {
                log::warn!("Unable to initialize EbpfLogger");
            }
        }

        EbpfFwdTables::new(
            &mut self.bpf.expect("Ebpf should have been set on build time"),
            self.root_id,
            self.next_hop_context,
            strategy,
            self.attach_type,
        )
    }
}
impl<H, E> EbpfFwdTablesBuilder<H, E>
where
    E: EbpfEntryStrategy,
    EbpfFwdTablesError: From<<E as EbpfEntryStrategy>::Error>,
{
    pub fn build(mut self) -> Result<EbpfFwdTables<H, E>, EbpfFwdTablesError> {
        let bpf = self
            .bpf
            .as_mut()
            .expect("Ebpf should have been set with `Self::bpf`");

        let entry_strategy = E::with_bpf(bpf)?;
        EbpfFwdTables::new(
            &mut self.bpf.expect("Ebpf should have been set on build time"),
            self.root_id,
            self.next_hop_context,
            entry_strategy,
            self.attach_type,
        )
    }
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
impl<H, E> EbpfFwdTables<H, E> {
    pub fn new(
        bpf: &mut Ebpf,
        root_id: NodeId,
        next_hop_context: H,
        entry_strategy: E,
        attach_type: XdpAttachType,
    ) -> Result<Self, EbpfFwdTablesError> {
        // TODO bump_memlock_rlimit

        // HACK obtain actual Xdp program
        let xdp = ebpf_utils::load_xdp(bpf)?;
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
    fn stats(&self) -> String {
        let xdp = self.xdp.xdp();
        let Ok(info) = xdp.info() else {
            return "{\n}".to_string();
        };
        let program_name = String::from_utf8_lossy(info.name());
        let run_count = info.run_count();
        let run_time_ns = info.run_time().as_nanos();

        format!("{{\n\"{program_name}\": {{\n\"run_time_ns\": {run_time_ns}\n\"run_count\": {run_count}\n}}")
    }
}
