import io
from dataclasses import dataclass, field
from typing import Any

import matplotlib.pyplot as plt
import networkx as nx

from kira_nest.nest.node import KIRANode
from kira_nest.nest.test import KIRATest

COLOR_VICINITY_NODE = "#8CB63C"  # kit-maygreen
COLOR_CONTACT_NODE = "#4664AA"  # kit-blue
COLOR_ROOT_NODE = "#009682"  # kit-green
COLOR_VICINITY_EDGE = "#8CB63C"  # kit-maygreen

COLOR_MISSING_VICINITY_NODE = "#A22223"  # kit-red
COLOR_MISSING_VICINITY_EDGE = "#A22223"  # kit-red

COLOR_MISSING_VICINITY_PATH = "#DF9B1B"  # kit-orange
COLOR_MISSING_CONTACT_PATH = "#A3107C"  # kit-purple


@dataclass
class KIRAImager[T]:
    test: KIRATest[T]
    _pos: Any = field(
        init=False,
        compare=False,
        default=None,
    )

    def __post_init__(self) -> None:
        # calculate initial positions
        self.update_pos()

    def update_pos(self) -> None:
        # keep pos of existing nodes but calculate new positions for new nodes
        self._pos = nx.kamada_kawai_layout(self.test.topology.topology, pos=self._pos)

    def image_topology(self, dpi: int = 200) -> io.BytesIO:
        nx.draw_networkx(self.test.topology.topology, pos=self._pos, font_color="w")

        buffer = io.BytesIO()
        plt.savefig(buffer, format="png", transparent=True, dpi=dpi)
        plt.clf()
        return buffer

    def image_vicinity(self, of: T, dpi: int = 200) -> io.BytesIO:
        topology = self.test.topology.topology
        node = self.test.topology.topology_nodes[of]
        vicinity = list(node.vicinity())
        vicinity_edges = list(node.vicinity_edges())
        root = of
        assert root == node.tid

        # draw other parts with their defaults first
        nx.draw_networkx(self.test.topology.topology, pos=self._pos, font_color="w")

        # draw the vicinity
        nx.draw_networkx_nodes(
            topology,
            pos=self._pos,
            nodelist=vicinity,
            node_color=COLOR_VICINITY_NODE,
        )
        nx.draw_networkx_nodes(
            topology, pos=self._pos, nodelist=[root], node_color=COLOR_ROOT_NODE
        )
        nx.draw_networkx_edges(
            topology,
            pos=self._pos,
            edgelist=vicinity_edges,
            edge_color=COLOR_VICINITY_EDGE,
            width=2,
        )

        buffer = io.BytesIO()
        plt.savefig(buffer, format="png", transparent=True, dpi=dpi)
        plt.clf()
        return buffer

    def image_node(self, node: KIRANode, dpi: int = 200) -> io.BytesIO:
        root = self.test.topology.tid(node)
        tnode = self.test.topology.topology_nodes[root]

        known_vicinity = tnode.known_vicinity(node)
        unknown_vicinity = tnode.unknown_vicinity(node)
        known_edges = tnode.known_vicinity_edges(node)
        unknow_edges = tnode.unknown_vicinity_edges(node)

        # draw other parts with their defaults first
        nx.draw_networkx(self.test.topology.topology, pos=self._pos, font_color="w")

        # draw the vicinity
        nx.draw_networkx_nodes(
            self.test.topology.topology,
            pos=self._pos,
            nodelist=list(known_vicinity),
            node_color=COLOR_VICINITY_NODE,
        )
        nx.draw_networkx_nodes(
            self.test.topology.topology,
            pos=self._pos,
            nodelist=list(unknown_vicinity),
            node_color=COLOR_MISSING_VICINITY_NODE,
        )
        nx.draw_networkx_nodes(
            self.test.topology.topology,
            pos=self._pos,
            nodelist=[root],
            node_color=COLOR_ROOT_NODE,
        )
        nx.draw_networkx_edges(
            self.test.topology.topology,
            pos=self._pos,
            edgelist=list(known_edges),
            edge_color=COLOR_VICINITY_EDGE,
            width=2,
        )
        nx.draw_networkx_edges(
            self.test.topology.topology,
            pos=self._pos,
            edgelist=list(unknow_edges),
            edge_color=COLOR_MISSING_VICINITY_EDGE,
            width=2,
        )

        buffer = io.BytesIO()
        plt.savefig(buffer, format="png", transparent=True, dpi=dpi)
        plt.clf()
        return buffer
