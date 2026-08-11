import os
from .node import KIRANode
from .network import KIRANetwork

import docker
from docker.models.networks import Network
from mininet.net import Containernet
import networkx as nx

from typing import Optional, TypeVar

Self = TypeVar("Self", bound="KIRAContainernetNode")


import logging

logger = logging.getLogger(__name__)


class KIRAContainernetNode(KIRANode):
    def __init__(
        self,
        net: Containernet,
        client=docker.from_env(),
        name: str = None,
        api_port: int = 8080,
    ):
        self._net = net
        self._canonical_name = name.replace("mn.", "")
        super().__init__(client, name, api_port)

    def create(
        self, img: str, nid: bytes = None, force: bool = False, privileged=False
    ):
        sysctls = {
            "net.ipv6.conf.default.disable_ipv6": 0,
            "net.ipv6.conf.all.forwarding": 1,
        }
        environment = []
        # rust log from system environment
        log_level = os.environ.get("RUST_LOG", "debug")
        environment.append(f"RUST_LOG={log_level}")
        if nid is not None:
            nid = nid[:14]
            environment.append(f"NODE_ID={nid.hex()}")

        cmd = "/usr/bin/supervisord -c /etc/supervisor/supervisord.conf"

        self._container = self._net.addDocker(
            self._canonical_name,
            ip=None,
            dimage=img,
            sysctls=sysctls,
            environment=environment,
            # WARNING: this doesn't actually work for now in containernet
            privileged=privileged,
            dcmd=cmd,
        )

    def connect_with(
        self, other: Self, id: str, nw: Optional[Network] = None
    ) -> Network:
        return self._net.addLink(self._container, other._container)

    def start(self):
        # can't start individual nodes
        # TODO: raise exception
        pass


class KIRAContainernetNetwork(KIRANetwork):
    _node_prefix = "mn.kira-n"  # used to find existing containers
    # TODO: get network name
    _nw_prefix = "TODO"

    def __init__(
        self, graph: nx.Graph, net: Containernet, client=docker.from_env(), seed=None
    ):
        self._net = net

        super().__init__(graph, client, seed, KIRAContainernetNode)

    def _init_node(self, n):
        name = f"{self._node_prefix}{n}"

        node = KIRAContainernetNode(self._net, client=self._client, name=name)
        self.graph.nodes[n][self._node_nx] = node

    def start(self):
        self._net.start()
