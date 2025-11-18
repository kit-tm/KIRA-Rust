from dataclasses import dataclass

from kira_nest.domain import NodeID, Path

# NOTE: Definitions don't align with kira-r2kad/src/domain/protocol_event/forwarding.rs
#
# This is because these are the actual elemental rules used by the netlink
# implementation to perform the forwarding.
# Since the PathID can easily be constructed from a Path but not the other way
# around (one-way hash function) we save the concrete path.
# Additionally we'd save the underlay neighbor destination of the forwarding
# action performed separately since it is common for all rules.


@dataclass
class NodeIDFwdEntry:
    """Forward the packet based on the NodeID to a underlay neighbor."""

    node: NodeID


@dataclass
class NodeIDEncapEntry:
    """
    Encapsulate the packet with a PathID.

    Further forwarding on the node is done solely based on the PathID.
    """

    node: NodeID
    path: Path


NodeIDEntry = NodeIDFwdEntry | NodeIDEncapEntry


@dataclass
class PathIDFwdEntry:
    """Forward the packet based on the PathID to a underlay neighbor."""

    path: Path


@dataclass
class PathIDSwapEntry:
    """
    Swap the PathID of the packet.

    Further forwarding on the node is done solely based on the resulting PathID.
    """

    in_path: Path
    out_path: Path


PathIDEntry = PathIDFwdEntry | PathIDSwapEntry


FwdEntry = NodeIDEntry | PathIDEntry
