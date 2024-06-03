from dataclasses import dataclass, field

from .node import Node
from mininet.net import Containernet
from networkx import Graph


@dataclass
class Network:
    G: Graph
    net: Containernet = field(repr=False)

    def __post_init__(self):
        for label in self.G:
            self.G.nodes[label]["container"] = Node.from_graph(self.G, label, self.net)

        for (u, v) in self.G.edges:
            self.net.addLink(u, v)
