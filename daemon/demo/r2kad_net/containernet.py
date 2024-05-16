import os
from .node import R2KadNode
from .network import R2KadNetwork

import docker
from docker.models.networks import Network
from mininet.net import Containernet
import networkx as nx

from typing import Optional, TypeVar
Self = TypeVar("Self", bound="R2KadContainernetNode")


class R2KadContainernetNode(R2KadNode):
    def __init__(self, net: Containernet, client=docker.from_env(), name: str = None, api_port: int = 8080):
        self._net = net
        self._cannonical_name = name.replace("mn.", "")
        super().__init__(client, name, api_port)

    def create(self, img: str, nid: bytes = None, force: bool = False):
        sysctls = {
            'net.ipv6.conf.default.disable_ipv6': 0,
            'net.ipv6.conf.all.forwarding': 1,
        }
        environment = []
        # rust log from system enviroment
        log_level = os.environ.get('RUST_LOG', 'debug')
        environment.append(f"RUST_LOG={log_level}")
        if nid is not None:
            nid = nid[:14]
            environment.append(f"NODE_ID={nid.hex()}")

        # FIXME why is this needed?
        cmd = "/usr/bin/supervisord -c /etc/supervisord.conf"

        self._container = self._net.addDocker(self._cannonical_name,
                                              ip=None, dimage=img,
                                              sysctls=sysctls,
                                              environment=environment,
                                              dcmd=cmd)

    def connect_with(self, other: Self, id: str, nw: Optional[Network] = None) -> Network:
        return self._net.addLink(self._cannonical_name, other._cannonical_name)

    def start(self):
        # can't start individual nodes
        # TODO raise exception
        pass


class R2KadContainernetNetwork(R2KadNetwork):
    _node_prefix = "mn.r2kad-n"  # used to find existing containers
    # TODO get network name
    _nw_prefix = "TODO"

    def __init__(self, graph: nx.Graph, net: Containernet, client=docker.from_env(), seed=None):
        self._net = net

        super().__init__(graph, client, seed, R2KadContainernetNode)

    def _init_node(self, n):
        name = f"{self._node_prefix}{n}"

        node = R2KadContainernetNode(self._net, client=self._client, name=name)
        self.graph.nodes[n][self._node_nx] = node

    def start(self):
        self._net.start()
