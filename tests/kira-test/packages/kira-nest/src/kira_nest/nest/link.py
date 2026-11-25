from __future__ import annotations

from typing import TYPE_CHECKING

from nest import engine

from kira_nest.nest.node import KIRANode

if TYPE_CHECKING:
    from nest.topology import Interface


class KIRALink:
    _is_up: bool
    _interface_x: Interface
    _interface_y: Interface

    def __init__(self, interface_x: Interface, interface_y: Interface) -> None:
        self._interface_x = interface_x
        self._interface_y = interface_y

        # just to be sure
        self.up()

    def down(self) -> None:
        self._interface_x.set_mode("DOWN")
        self._interface_y.set_mode("DOWN")
        self._is_up = False

    def up(self) -> None:
        self._interface_x.set_mode("UP")
        self._interface_y.set_mode("UP")
        self._is_up = True

    def is_up(self) -> bool:
        return self._is_up

    def is_down(self) -> bool:
        return not self._is_up

    def is_local(self) -> bool:
        """Whether this host emulates both ends of the link."""
        return True

    def id(self, of: KIRANode) -> str | None:
        if self._interface_x in of.interfaces:
            assert self._interface_y.node_id not in of.interfaces
            return self._interface_x.id
        elif self._interface_y.node_id in of.interfaces:
            return self._interface_y.id
        else:
            return None


class VxlanLink(KIRALink):
    """A link to a node on another host, over a VXLAN interface of the local node.

    Only the local end is set up or down. The other host keeps its end up, but no
    packet passes the link in either direction while the local end is down. The
    other host's view of the topology doesn't learn about the change.
    """

    def __init__(self, node: KIRANode, ifname: str, remote_host: str) -> None:
        self._node = node
        self._ifname = ifname
        self.remote_host = remote_host
        self._is_up = True

    def down(self) -> None:
        engine.set_interface_mode(self._node.id, self._ifname, "down")
        self._is_up = False

    def up(self) -> None:
        engine.set_interface_mode(self._node.id, self._ifname, "up")
        self._is_up = True

    def is_local(self) -> bool:
        return False

    def id(self, of: KIRANode) -> str | None:
        return self._ifname if of == self._node else None


class ForeignLink(KIRALink):
    """A link between two nodes emulated on other hosts.

    It is assumed to be up: the state is controlled on the other hosts.
    """

    def __init__(self, host_x: str, host_y: str) -> None:
        self.hosts = (host_x, host_y)
        self._is_up = True

    def down(self) -> None:
        raise RuntimeError(f"link between nodes on hosts {self.hosts} is not local")

    def up(self) -> None:
        raise RuntimeError(f"link between nodes on hosts {self.hosts} is not local")

    def is_local(self) -> bool:
        return False

    def id(self, of: KIRANode) -> str | None:
        _ = of
        return None
