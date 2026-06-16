from __future__ import annotations

from hashlib import sha1
from ipaddress import IPv6Address, IPv6Network
from typing import Any, Self

NODE_IP_SN: IPv6Network = IPv6Network("fc00::/16")
PATH_IP_SN: IPv6Network = IPv6Network("fcaa::/16")

VICINITY_RADIUS: int = 2


class NodeIP(IPv6Address):
    def __init__(self, address: object) -> None:
        super().__init__(address)

        if self not in NODE_IP_SN:
            raise ValueError(f"NodeIP not in {NODE_IP_SN}")

    def to_node_id(self) -> NodeID:
        return NodeID(self.packed[2:])


class PathIP(IPv6Address):
    def __init__(self, address: object) -> None:
        super().__init__(address)

        if self not in PATH_IP_SN:
            raise ValueError(f"PathIP not in {PATH_IP_SN}")

    def to_path_id(self) -> PathID:
        return PathID(self.packed[2:])


KiraIP = NodeIP | PathIP


class NodeID(bytes):
    LENGTH = 14
    PRFX = bytes.fromhex("fc00")

    def __new__(cls, *args: Any, **kwargs: Any) -> Self:
        tmp_bytes = bytes(*args, **kwargs)
        if len(tmp_bytes) != cls.LENGTH:
            raise ValueError(f"NodeID must have a length of {cls.LENGTH} bytes")

        return super().__new__(cls, *args, **kwargs)

    def to_node_ip(self) -> NodeIP:
        return NodeIP(self.PRFX + self)

    def __str__(self) -> str:
        return self.hex()


class PathID(bytes):
    LENGTH = 14
    PRFX = bytes.fromhex("fcaa")

    def __new__(cls, *args: Any, **kwargs: Any) -> Self:
        tmp_bytes = bytes(*args, **kwargs)
        if len(tmp_bytes) != cls.LENGTH:
            raise ValueError(f"PathID must have a length of {cls.LENGTH} bytes")

        return super().__new__(cls, *args, **kwargs)

    def to_path_ip(self) -> PathIP:
        return PathIP(self.PRFX + self)

    def __str__(self) -> str:
        return self.hex()


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
        path_id = sha1(b"".join(self)).digest()[:14]
        return PathID(path_id)

    def to_path_ip(self) -> PathIP:
        return self.to_path_id().to_path_ip()

    def __str__(self) -> str:
        return ", ".join(str(n) for n in self)


class SourceRoute(Path):  # progress
    def source(self) -> NodeID:
        return self[0]
