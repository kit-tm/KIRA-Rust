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
    def __getattr__(self, name):
        return getattr(self._container, name)

    def __setattr__(self, name, value):
        if name in ['_container', 'node_config', 'net']:
            super().__setattr__(name, value)
        else:
            setattr(self._container, name, value)

    def __delattr__(self, name):
        if name in ['_container', 'node_config', 'net']:
            super().__delattr__(name)
        else:
            delattr(self._container, name)
