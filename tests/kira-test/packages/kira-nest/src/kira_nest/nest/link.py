from __future__ import annotations

from typing import TYPE_CHECKING

from kira_nest.nest.node import KIRANode

if TYPE_CHECKING:
    from nest.topology import Interface


class KIRALink:
    _is_up: bool
    _interface_x: Interface
    _interface_y: Interface

    def __init__(self, inteface_x: Interface, interface_y: Interface) -> None:
        self._interface_x = inteface_x
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

    def id(self, of: KIRANode) -> str | None:
        if self._interface_x in of.interfaces:
            assert self._interface_y.node_id not in of.interfaces
            return self._interface_x.id
        elif self._interface_y.node_id in of.interfaces:
            return self._interface_y.id
        else:
            return None
