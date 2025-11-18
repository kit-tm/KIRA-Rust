from __future__ import annotations

from collections.abc import Iterator
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any, cast

import networkx as nx

from kira_nest.domain import VICINITY_RADIUS
from kira_nest.nest.node import KIRANode

if TYPE_CHECKING:
    from kira_nest.domain.topology import KIRATopology


@dataclass(frozen=True)
class KIRATopologyNode[T]:
    tid: T
    topology: KIRATopology

    def __str__(self) -> str:
        return f"{self.__class__.__name__}({self.tid})"

    def _vicinity_hc(self) -> dict[T, int]:
        hopcounts = nx.single_source_dijkstra_path_length(
            self.topology.topology,
            cast(Any, self.tid),  # typing seems to be off
            # asking for neighbors inside of nodes _inside_ the vicinity
            # but not adding them to vicinity graph if on the edge
            cutoff=VICINITY_RADIUS,
            weight=lambda x, y, _: self.topology.links.weight(
                x, y
            ),  # hide edges currently unconnected
        )
        return {v: int(hc) for v, hc in hopcounts.items() if v != self.tid}

    def vicinity(self) -> Iterator[T]:
        return iter(self._vicinity_hc().keys())

    def vicinity_edges(self) -> Iterator[tuple[T, T]]:
        vicinity = self._vicinity_hc()

        # edges from root are part of the vicinity edges
        # although root is not part of the vicinity itself
        vicinity[self.tid] = 0

        for u, v, link in self.topology.links:
            if link.is_down():
                continue

            uhc = vicinity.get(u)
            vhc = vicinity.get(v)
            if uhc is None or vhc is None:
                continue
            assert uhc <= VICINITY_RADIUS
            assert vhc <= VICINITY_RADIUS

            # no links between edge nodes
            if uhc == VICINITY_RADIUS and vhc == VICINITY_RADIUS:
                continue

            yield (u, v)

    def vicinity_paths(self) -> Iterator[list[T]]:
        for v in self.vicinity():
            for path in nx.shortest_simple_paths(
                self.topology.topology,
                self.tid,
                v,
                weight=lambda x, y, _: self.topology.links.weight(
                    x, y
                ),  # hide edges currently unconnected
            ):
                assert len(path) > 1  # vicinity shouldn't include us
                path_to_v = path[1:]  # exclude us
                if len(path_to_v) > VICINITY_RADIUS:
                    break  # because paths are returned ordered by length
                yield path_to_v

    def known_vicinity(self, of: KIRANode) -> Iterator[T]:
        known_vicinity = of.vicinity()
        if known_vicinity is None:
            return iter([])
        return map(lambda x: self.topology.topology_nodes[x].tid, known_vicinity)

    def known_vicinity_edges(self, of: KIRANode) -> Iterator[tuple[T, T]]:
        known_vicinity_edges = of.vicinity_edges()
        if known_vicinity_edges is None:
            return iter([])
        return map(
            lambda edge: (
                self.topology.topology_nodes[edge[0]].tid,
                self.topology.topology_nodes[edge[1]].tid,
            ),
            known_vicinity_edges,
        )

    def unknown_vicinity(self, of: KIRANode) -> Iterator[T]:
        topo_vicinity = self.vicinity()
        known_vicinity = set(self.known_vicinity(of))

        for vicinity_node in topo_vicinity:
            if vicinity_node not in known_vicinity:
                yield vicinity_node

    def unknown_vicinity_edges(self, of: KIRANode) -> Iterator[tuple[T, T]]:
        topo_vicinity_edges = self.vicinity_edges()
        known_vicinity_edges = set(self.known_vicinity_edges(of))

        for x, y in topo_vicinity_edges:
            if {(x, y), (y, x)}.isdisjoint(known_vicinity_edges):
                yield (x, y)

    def additional_vicinity(self, of: KIRANode) -> Iterator[T]:
        topo_vicinity = self.vicinity()
        known_vicinity = of.vicinity()
        if known_vicinity is None:
            return iter([])
        known_vicinity = map(
            lambda x: self.topology.topology_nodes[x].tid, known_vicinity
        )

        topo_vicinity = set(topo_vicinity)
        for vicinity_node in known_vicinity:
            if vicinity_node not in topo_vicinity:
                yield vicinity_node.tid
