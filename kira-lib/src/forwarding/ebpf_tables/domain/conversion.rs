//! Implementation of conversions from internal data types
//! to types used in the eBPF-Maps used to store the Forwarding-Table data

use std::array::TryFromSliceError;

use crate::domain as kira_lib;
use kira_bpf_common::domain::kira::{forwarding::maps::NodeIdSubnet, NodeId, PathId, SIZE};

use crate::forwarding::{
    NodeIdEncapsulationEntry, NodeIdEntry, NodeIdForwardingEntry, PathIdDecapsulationEntry,
    PathIdEntry, PathIdForwardingEntry,
};

impl From<kira_lib::NodeId> for NodeId {
    fn from(value: kira_lib::NodeId) -> Self {
        Self::from(value.bytes)
    }
}

impl TryFrom<kira_lib::PathId> for PathId {
    type Error = TryFromSliceError;

    fn try_from(value: kira_lib::PathId) -> Result<Self, Self::Error> {
        value.as_ref().try_into()
    }
}

impl From<kira_lib::NodeIdSubnet> for NodeIdSubnet {
    fn from(value: kira_lib::NodeIdSubnet) -> Self {
        Self::new(
            value
                .prefix_length
                .try_into()
                .expect("usize into u32 prefix value should be possible"),
            value.node_id.into(),
        )
    }
}

impl NodeIdEntry {
    pub(in crate::forwarding::ebpf_tables) fn destination(self) -> NodeIdSubnet {
        let (Self::Forward(NodeIdForwardingEntry { destination, .. })
        | Self::Encapsulate(NodeIdEncapsulationEntry { destination, .. })) = self;

        destination.into()
    }

    pub(in crate::forwarding::ebpf_tables) fn out_path_id(self) -> Option<PathId> {
        match self {
            Self::Forward(_) => None,
            Self::Encapsulate(NodeIdEncapsulationEntry { out_path_id, .. }) => {
                let out_path_id = out_path_id
                    .try_into()
                    .expect("PathId should be properly sized");
                Some(out_path_id)
            }
        }
    }
}
impl PathIdEntry {
    pub(in crate::forwarding::ebpf_tables) fn in_path_id(self) -> PathId {
        let (Self::Forward(PathIdForwardingEntry { in_path_id, .. })
        | Self::Decapsulate(PathIdDecapsulationEntry { in_path_id, .. })) = self;

        in_path_id
            .try_into()
            .expect("PathId should be properly sized")
    }

    pub(in crate::forwarding::ebpf_tables) fn out_path_id(self) -> Option<PathId> {
        match self {
            Self::Decapsulate(_) => None,
            Self::Forward(PathIdForwardingEntry { out_path_id, .. }) => {
                let out_path_id = out_path_id
                    .try_into()
                    .expect("PathId should be properly sized");
                Some(out_path_id)
            }
        }
    }
}
