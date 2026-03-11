from __future__ import annotations

from collections.abc import Iterator
from dataclasses import dataclass, field
from functools import cached_property, singledispatchmethod
from types import EllipsisType
from typing import ClassVar, overload

from kira_common.domain import NodeID, NodeIP
from kira_common.node_config import NodeConfig
from networkx import Graph
from networkx.classes.reportviews import OutEdgeView

from kira_nest.nest.link import KIRALink
from kira_nest.nest.node import KIRANode
from kira_nest.topology import KIRATopologyNode

# TODO: This should probably reside in kira_common and be independent of NeST


@dataclass
class KIRANodeView[T: str | int]:
    _NODE_ID: ClassVar[str] = "node"
    _inner: KIRATopology[T]

    @singledispatchmethod
    def __getitem__(self, key: T | NodeID | NodeIP) -> KIRANode:
        _ = key
        raise NotImplementedError("__getitem__ only implemented for TID, NID and NIP")

    @__getitem__.register
    def by_tid(self, key: str | int) -> KIRANode:
        node = self._inner.topology.nodes[key][self._NODE_ID]
        if node is None:
            raise KeyError
        assert type(node) is KIRANode
        return node

    @__getitem__.register
    def by_nid(self, key: NodeID) -> KIRANode:
        for n in self:
            if n.node_id == key:
                return n
        raise KeyError

    @__getitem__.register
    def by_nip(self, key: NodeIP) -> KIRANode:
        for n in self:
            if n.node_id.to_node_ip() == key:
                return n
        raise KeyError

    def by_name(self, key: str) -> KIRANode | None:
        tid = self._inner._name_tid_mapping.get(key)
        if tid is None:
            return None
        return self[tid]

    def get[D](self, key: T | NodeID | NodeIP | str, default: D = None) -> KIRANode | D:
        try:
            return self[key]
        except KeyError:
            return default

    def __setitem__(self, key: T, value: KIRANode) -> None:
        self._inner.topology.nodes[key][self._NODE_ID] = value
        self._inner._name_tid_mapping[value._name] = key

    def __delitem__(self, key: T) -> None:
        name = self[T].name
        del self._inner._name_tid_mapping[name]
        del self._inner.topology.nodes[key][self._NODE_ID]

    def __iter__(self) -> Iterator[KIRANode]:  # all initialized KIRANodes
        for _, node in self._inner.topology.nodes.data(self._NODE_ID):
            if node is not None:
                assert type(node) is KIRANode
                yield node


@dataclass
class KIRATopologyNodeView[T: str | int]:
    _NODE_ID: ClassVar[str] = "node"
    _inner: KIRATopology[T]

    def get[D](self, key: T, default: D = None) -> KIRATopologyNode | D:
        try:
            return self[key]
        except KeyError:
            return default

    @singledispatchmethod
    def __getitem__(self, key: T | NodeID | NodeIP) -> KIRATopologyNode:
        _ = key
        raise NotImplementedError("__getitem__ only implemented for TID, NID and NIP")

    @__getitem__.register
    def by_tid(self, key: str | int) -> KIRATopologyNode:
        return KIRATopologyNode(key, self._inner)

    @__getitem__.register
    def by_nid(self, key: NodeID) -> KIRATopologyNode:
        for tid, cfg in self._inner.configs():
            if NodeID.fromhex(cfg.node_id) == key:
                return self[tid]
        raise KeyError

    @__getitem__.register
    def by_nip(self, key: NodeIP) -> KIRATopologyNode:
        for tid, cfg in self._inner.configs():
            if NodeIP(cfg.ipv6) == key:
                return self[tid]
        raise KeyError

    def __iter__(self) -> Iterator[KIRATopologyNode]:
        for n in self._inner.topology:
            yield KIRATopologyNode(n, self._inner)


@dataclass
class KIRALinkView[T: str | int]:
    _LINK_ID: ClassVar[str] = "link"
    _inner: KIRATopology[T]

    def links_of(self, of: T | None) -> Iterator[tuple[T, KIRALink]]:
        return (
            (tid, link)
            for _, tid, link in self._inner.topology.edges(of, data=self._LINK_ID)
        )

    def get[D](self, x: T, y: T, default: D = None) -> KIRALink | D:
        try:
            return self[x, y]
        except KeyError:
            return default

    def weight(self, x: T, y: T) -> int | None:
        """
        Returns one if a link between x and y exists and is up otherwise None.

        This function can be used as `weights` argument in NetworkX functions
        to hide edges in the topology where no KIRALink exists or is down.
        """

        link = self.get(x, y)
        if link is not None and link.is_up():
            return 1

        return None  # disable edge

    @overload
    def __getitem__(
        self, key: tuple[T, EllipsisType]
    ) -> Iterator[tuple[T, KIRALink]]: ...

    @overload
    def __getitem__(
        self, key: tuple[EllipsisType, T]
    ) -> Iterator[tuple[T, KIRALink]]: ...

    @overload
    def __getitem__(self, key: tuple[T, T]) -> KIRALink: ...

    def __getitem__(self, key):
        x_tid, y_tid = key
        if x_tid is ...:
            assert y_tid is not ...
            return self.links_of(y_tid)
        if y_tid is ...:
            assert x_tid is not ...
            return self.links_of(x_tid)

        return self._inner.topology.edges[x_tid, y_tid][self._LINK_ID]

    def __setitem__(self, key: tuple[T, T], value: KIRALink) -> None:
        x_tid, y_tid = key
        self._inner.topology.edges[x_tid, y_tid][self._LINK_ID] = value

    def __delitem__(self, key: tuple[T, T]) -> None:
        x_tid, y_tid = key
        del self._inner.topology.edges[x_tid, y_tid][self._LINK_ID]

    def __iter__(self) -> Iterator[tuple[T, T, KIRALink]]:
        for x_tid, y_tid, link in self._inner.topology.edges.data(self._LINK_ID):
            if link is not None:
                yield x_tid, y_tid, link


@dataclass
class KIRATopology[T: str | int]:
    _CONFIG_ID: ClassVar[str] = "config"
    topology: Graph

    def __post_init__(self):
        # init config
        for tid, raw_cfg in self.topology.nodes(data=self._CONFIG_ID):
            cfg = NodeConfig(**raw_cfg)
            self.topology.nodes[tid][self._CONFIG_ID] = cfg

    _name_tid_mapping: dict[str, T] = field(
        init=False,
        compare=False,  # no need since name is also stored in the Graph
        default_factory=dict,
    )

    def tid(self, node: KIRANode) -> T | None:
        name = node.name
        return self._name_tid_mapping.get(name)

    @cached_property
    def nodes(self) -> KIRANodeView[T]:
        return KIRANodeView(self)

    @cached_property
    def topology_nodes(self) -> KIRATopologyNodeView[T]:
        return KIRATopologyNodeView(self)

    @cached_property
    def links(self) -> KIRALinkView[T]:
        return KIRALinkView(self)

    def configs(self) -> Iterator[tuple[T, NodeConfig]]:
        for tid, cfg in self.topology.nodes.data(self._CONFIG_ID):
            assert isinstance(cfg, NodeConfig)
            yield (tid, cfg)

    def edges(self) -> OutEdgeView:
        return self.topology.edges
