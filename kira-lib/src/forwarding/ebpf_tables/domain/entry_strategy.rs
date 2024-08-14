use std::borrow::BorrowMut;

use kira_bpf_common::aya::maps::{MapData, MapError};

use kira_bpf_common::aya::Bpf;
use kira_bpf_common::domain::kira as kira_bpf;
use kira_bpf_common::domain::kira::forwarding::maps;
use kira_bpf_common::domain::kira::forwarding::maps::NextHop;
pub use kira_bpf_common::domain::kira::forwarding::TablesHandle; // make usable

use crate::domain::{NodeIdSubnet, PathId};
use crate::forwarding::ebpf_tables::domain::context::PathIdEntryContext;
use crate::forwarding::ebpf_tables::EbpfFwdTablesError;
use crate::forwarding::{NodeIdEntry, PathIdEntry};

pub trait NodeIdEntryStrategy {
    type Error;
    type Context;

    fn create(&mut self, entry: NodeIdEntry, context: Self::Context) -> Result<(), Self::Error>;
    fn update(&mut self, entry: NodeIdEntry, context: Self::Context) -> Result<(), Self::Error>;
    fn create_or_update(
        &mut self,
        entry: NodeIdEntry,
        context: Self::Context,
    ) -> Result<(), Self::Error>;

    fn remove(&mut self, node_id: &NodeIdSubnet) -> Result<Option<NodeIdEntry>, Self::Error>;
}

pub trait PathIdEntryStrategy {
    type Error;
    type Context;

    fn create(&mut self, entry: PathIdEntry, context: Self::Context) -> Result<(), Self::Error>;
    fn update(&mut self, entry: PathIdEntry, context: Self::Context) -> Result<(), Self::Error>;
    fn create_or_update(
        &mut self,
        entry: PathIdEntry,
        context: Self::Context,
    ) -> Result<(), Self::Error>;

    fn remove(&mut self, path_id: &PathId) -> Result<Option<PathIdEntry>, Self::Error>;
}

impl<T> NodeIdEntryStrategy for TablesHandle<T>
where
    T: BorrowMut<MapData>,
{
    type Error = MapError;

    type Context = NextHop;

    fn create(&mut self, entry: NodeIdEntry, context: Self::Context) -> Result<(), Self::Error> {
        let at = entry.clone().destination();
        let out_path_id = entry.out_path_id();
        let entry = maps::NodeIdEntry {
            out_path_id,
            next_hop: context,
        };

        self.create_nid_entry(&at, &entry)
    }

    fn update(&mut self, entry: NodeIdEntry, context: Self::Context) -> Result<(), Self::Error> {
        let at = entry.clone().destination();
        let out_path_id = entry.out_path_id();
        let entry = maps::NodeIdEntry {
            out_path_id,
            next_hop: context,
        };

        self.update_nid_entry(&at, &entry)
    }

    fn create_or_update(
        &mut self,
        entry: NodeIdEntry,
        context: Self::Context,
    ) -> Result<(), Self::Error> {
        let at = entry.clone().destination();
        let out_path_id = entry.out_path_id();
        let entry = maps::NodeIdEntry {
            out_path_id,
            next_hop: context,
        };

        self.create_or_update_nid_entry(&at, &entry)
    }

    fn remove(&mut self, node_id: &NodeIdSubnet) -> Result<Option<NodeIdEntry>, Self::Error> {
        let at = node_id.clone().into();

        self.remove_nid_entry(&at)?;

        Ok(None) // it is not possible for eBPF-LpmTries to return the removed value
    }
}

impl<T> PathIdEntryStrategy for TablesHandle<T>
where
    T: BorrowMut<MapData>,
{
    type Error = MapError;

    type Context = PathIdEntryContext;

    fn create(&mut self, entry: PathIdEntry, context: Self::Context) -> Result<(), Self::Error> {
        let at = entry.clone().in_path_id();
        let entry = path_id_entry_with_context(entry, context);

        self.create_pid_entry(&at.into(), &entry)
    }

    fn update(&mut self, entry: PathIdEntry, context: Self::Context) -> Result<(), Self::Error> {
        let at = entry.clone().in_path_id();
        let entry = path_id_entry_with_context(entry, context);

        self.update_pid_entry(&at.into(), &entry)
    }

    fn create_or_update(
        &mut self,
        entry: PathIdEntry,
        context: Self::Context,
    ) -> Result<(), Self::Error> {
        let at = entry.clone().in_path_id();
        let entry = path_id_entry_with_context(entry, context);

        self.create_or_update_pid_entry(&at.into(), &entry)
    }

    fn remove(&mut self, path_id: &PathId) -> Result<Option<PathIdEntry>, Self::Error> {
        let at: kira_bpf::PathId = path_id
            .clone()
            .try_into()
            .expect("PathId should be properly sized");

        self.remove_pid_entry(&(at.into()))?;

        Ok(None) // it is not possible for eBPF-LpmTries to return the removed value
    }
}

fn path_id_entry_with_context(
    entry: PathIdEntry,
    context: PathIdEntryContext,
) -> maps::PathIdEntry {
    match context {
        PathIdEntryContext::NextHop(next_hop) => {
            let out_path_id = entry.out_path_id();
            maps::PathIdEntry {
                path_id_entry: Some(maps::PathIdForwardingEntry {
                    out_path_id,
                    next_hop,
                }),
            }
        }
        PathIdEntryContext::EndOfPath() => maps::PathIdEntry {
            path_id_entry: None,
        },
    }
}

pub trait EbpfEntryStrategy:
    PathIdEntryStrategy<Error = <Self as EbpfEntryStrategy>::Error>
    + NodeIdEntryStrategy<Error = <Self as EbpfEntryStrategy>::Error>
    + Sized
{
    type Error;
    fn with_bpf(bpf: &mut Bpf) -> Result<Self, <Self as EbpfEntryStrategy>::Error>;
}

impl EbpfEntryStrategy for TablesHandle<MapData> {
    type Error = MapError;

    fn with_bpf(bpf: &mut Bpf) -> Result<Self, <Self as EbpfEntryStrategy>::Error> {
        Ok(Self::new(bpf)?)
    }
}
