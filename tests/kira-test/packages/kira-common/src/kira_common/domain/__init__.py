from __future__ import annotations

import os
import random
from dataclasses import dataclass
from hashlib import shake_256
from ipaddress import IPv6Address, IPv6Network
from typing import Any, Self

# NodeIDs and PathIDs fill the remaining 112 bits of NodeIPs and PathIPs
IP_PREFIX_LENGTH: int = 16
DEFAULT_NODE_IP_PREFIX: IPv6Network = IPv6Network("fc00::/16")
DEFAULT_PATH_IP_PREFIX: IPv6Network = IPv6Network("fcaa::/16")
# same environment variables as used by kirad
ENV_NODE_IP_PREFIX: str = "NODE_IP_PREFIX"
ENV_PATH_IP_PREFIX: str = "PATH_IP_PREFIX"


def parse_ip_prefix(value: str | IPv6Network) -> IPv6Network:
    """Parses a /16 IPv6 prefix like kirad does, the prefix length may be omitted."""
    if isinstance(value, str) and "/" not in value:
        value = f"{value}/{IP_PREFIX_LENGTH}"
    # raises ValueError if bits after the prefix are set
    prefix = IPv6Network(value)
    if prefix.prefixlen != IP_PREFIX_LENGTH:
        raise ValueError(
            f"prefix length must be {IP_PREFIX_LENGTH}, got {prefix.prefixlen}"
        )
    return prefix


@dataclass(frozen=True)
class IPPrefixes:
    """Prefixes of NodeIPs and PathIPs, which must match the ones used by kirad."""

    node_ip: IPv6Network = DEFAULT_NODE_IP_PREFIX
    path_ip: IPv6Network = DEFAULT_PATH_IP_PREFIX

    def __post_init__(self) -> None:
        object.__setattr__(self, "node_ip", parse_ip_prefix(self.node_ip))
        object.__setattr__(self, "path_ip", parse_ip_prefix(self.path_ip))
        if self.node_ip == self.path_ip:
            raise ValueError(
                f"NodeIP and PathIP prefix must differ, both are {self.node_ip}"
            )

    @classmethod
    def from_env(cls) -> IPPrefixes:
        """Reads the prefixes from the environment variables also used by kirad."""
        return cls(
            os.environ.get(ENV_NODE_IP_PREFIX, DEFAULT_NODE_IP_PREFIX),
            os.environ.get(ENV_PATH_IP_PREFIX, DEFAULT_PATH_IP_PREFIX),
        )

    def is_default(self) -> bool:
        return self == IPPrefixes()

    def kirad_args(self) -> list[str]:
        """Returns the kirad arguments to use these prefixes."""
        return [
            "--node-ip-prefix",
            str(self.node_ip),
            "--path-ip-prefix",
            str(self.path_ip),
        ]


_ip_prefixes: IPPrefixes = IPPrefixes.from_env()


def get_ip_prefixes() -> IPPrefixes:
    """Returns the prefixes currently used to derive and check NodeIPs and PathIPs."""
    return _ip_prefixes


def set_ip_prefixes(prefixes: IPPrefixes) -> None:
    """Sets the prefixes used to derive and check NodeIPs and PathIPs."""
    global _ip_prefixes  # noqa: PLW0603
    _ip_prefixes = prefixes


VICINITY_RADIUS: int = 2


class NodeIP(IPv6Address):
    def __init__(self, address: object) -> None:
        super().__init__(address)

        prefix = get_ip_prefixes().node_ip
        if self not in prefix:
            raise ValueError(f"NodeIP not in {prefix}")

    def to_node_id(self) -> NodeID:
        return NodeID(self.packed[2:])


class PathIP(IPv6Address):
    def __init__(self, address: object) -> None:
        super().__init__(address)

        prefix = get_ip_prefixes().path_ip
        if self not in prefix:
            raise ValueError(f"PathIP not in {prefix}")

    def to_path_id(self) -> PathID:
        return PathID(self.packed[2:])


KiraIP = NodeIP | PathIP


class NodeID(bytes):
    LENGTH = 14

    def __new__(cls, *args: Any, **kwargs: Any) -> Self:
        tmp_bytes = bytes(*args, **kwargs)
        if len(tmp_bytes) != cls.LENGTH:
            raise ValueError(f"NodeID must have a length of {cls.LENGTH} bytes")

        return super().__new__(cls, *args, **kwargs)

    def to_node_ip(self) -> NodeIP:
        prefix = get_ip_prefixes().node_ip.network_address.packed[:2]
        return NodeIP(prefix + self)

    @classmethod
    def random(cls) -> Self:
        return cls(random.randbytes(cls.LENGTH))

    def __str__(self) -> str:
        return self.hex()


class PathID(bytes):
    LENGTH = 14

    def __new__(cls, *args: Any, **kwargs: Any) -> Self:
        tmp_bytes = bytes(*args, **kwargs)
        if len(tmp_bytes) != cls.LENGTH:
            raise ValueError(f"PathID must have a length of {cls.LENGTH} bytes")

        return super().__new__(cls, *args, **kwargs)

    def to_path_ip(self) -> PathIP:
        prefix = get_ip_prefixes().path_ip.network_address.packed[:2]
        return PathIP(prefix + self)

    def __str__(self) -> str:
        return self.hex()


# Environment variable selecting the encapsulation of the fast forwarding of kirad,
# which inherits the environment.
ENV_FORWARDING = "KIRA_FORWARDING"


def forwarding_mode() -> str:
    """Returns the encapsulation used by the fast forwarding: `gre` or `srv6`."""
    return os.environ.get(ENV_FORWARDING, "gre").lower()


def two_path_segments() -> bool:
    """Returns if paths are split into up to two path segments (SRv6 forwarding)."""
    return forwarding_mode() == "srv6"


# Longest path in hops that is covered by a single path segment.
# Path-IDs of these paths are precomputed by all nodes on the path.
MAX_SINGLE_SEGMENT_HOPS = 3


class Path(list[NodeID]):
    """
    A Path is a list of nodes and excludes the starting node.

    If you want to encode the starting node too you can use the SourceRoute class.
    """

    def __init__(self, *args: Any, **kwargs: Any) -> None:
        super().__init__(*args, **kwargs)

        if len(self) == 0:
            raise ValueError(
                "Invariant violated: A valid Path is not empty at any time."
            )

    def destination(self) -> NodeID:
        return self[-1]

    def to_path_id(self) -> PathID:
        """Calculate the Path-ID from a path of NodeIds."""
        path_id = shake_256(b"".join(self)).digest(14)
        return PathID(path_id)

    def to_path_ip(self) -> PathIP:
        return self.to_path_id().to_path_ip()

    def segments(self) -> tuple[Path, Path | None]:
        """
        Splits the path into the path segments used by the forwarding tier.

        Paths of up to three hops consist of a single segment. Longer paths have
        a second segment of two hops (paths of four and five hops) or three hops.
        The second segment starts with the last node of the first segment,
        as its Path-ID is the incoming one at this node.

        With GRE forwarding the whole path is always a single segment.
        """
        hops = len(self)
        if hops <= MAX_SINGLE_SEGMENT_HOPS or not two_path_segments():
            return self, None
        if hops < 2 * MAX_SINGLE_SEGMENT_HOPS:
            first_hops = hops - (MAX_SINGLE_SEGMENT_HOPS - 1)
        else:
            first_hops = hops - MAX_SINGLE_SEGMENT_HOPS
        return Path(self[:first_hops]), Path(self[first_hops - 1 :])

    def __str__(self) -> str:
        return ", ".join(str(n) for n in self)


class SourceRoute(Path):  # progress
    def source(self) -> NodeID:
        return self[0]
