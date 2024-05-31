from dataclasses import dataclass, field

from .node_config import NodeConfig
from mininet.net import Containernet
from networkx import Graph


@dataclass
class Node:
    node_config: NodeConfig
    net: Containernet = field(repr=False)

    def __post_init__(self):
        self._container = self.net.addDocker(self.node_config.name,
                                             ip=None, network_mode="none",
                                             dimage=self.node_config.docker_image, dcmd=self.node_config.dcmd,
                                             sysctls=self.node_config.sysctls, environment=self.node_config.enviroments)

    def save_in_graph(self, G: Graph, label):
        self.node_config.save_in_graph(G, label)

    @classmethod
    def from_graph(cls, G: Graph, label, net):
        node_config = NodeConfig.from_graph(G, label)
        return cls(node_config, net)

    # delegate all future calls to container so it can be used
    # as if it's a real container in Containernet
    def __getattr__(self, attr):
        return getattr(self._container, attr)

    def __setattr__(self, attr, value):
        setattr(self._container, attr)
