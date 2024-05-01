import docker
import networkx as nx

import random
from typing import Callable, Any

from .node import R2KadNode

import logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)


class R2KadNetwork:
    # ids used for storage in the graph
    _node_nx = "node"
    _network_nx = "network"

    _node_prefix = "r2kad-n"
    _nw_prefix = "r2edge-"

    def __init__(self, graph: nx.Graph, client=docker.from_env(), seed=None, node_type=R2KadNode):
        self.graph = graph
        self._client = client
        self._node_type = node_type

        if seed is None:
            seed = random.SystemRandom().randint(0, 1024)
            logger.info(f"Using Seed {seed} to generate ids of nodes")
        self._rng = random.Random(seed)

        for n in self.graph:
            self._init_node(n)

        networks = self._client.networks.list()
        for (u, v) in self.graph.edges:
            self._init_existing_network(u, v, networks)

    def _init_node(self, n):
        name = f"{self._node_prefix}{n}"

        node = self._node_type(client=self._client, name=name)
        self.graph.nodes[n][self._node_nx] = node

    def _init_existing_network(self, u, v, networks):
        for nw in networks:
            if nw.name == f"{self._nw_prefix}{u}-{v}":
                self.graph.edges[u, v][self._network_nx] = nw
                break

    def _for_all_nodes(self, fn: Callable[[R2KadNode], Any]):
        for (_, node) in self.graph.nodes(data=self._node_nx):
            fn(node)

    def get_node(self, node: int) -> R2KadNode:
        return self.graph.nodes[str(node)][self._node_nx]

    def create(self, img: str):
        self._for_all_nodes(lambda n: n.create(
            img, self._rng.getrandbits(14*8).to_bytes(14, "big")))

    def start(self):
        self._for_all_nodes(self._node_type.start)

    def stop(self):
        self._for_all_nodes(self._node_type.stop)

    def prune(self):
        self._for_all_nodes(self._node_type.prune)

        # prune network
        for (_, _, nw) in self.graph.edges.data(data=self._network_nx):
            if nw is not None:
                nw.remove()

    def connect(self):
        for (u, v, nw) in self.graph.edges.data(data=self._network_nx):
            nu = self.get_node(u)
            nv = self.get_node(v)
            id = f"{self._nw_prefix}{u}-{v}"
            nw = nu.connect_with(nv, id, nw=nw)

    def test_connectivity(self, origin: int = None):
        all_connected = True
        if origin is None:
            origins = self.graph
        else:
            origins = [origin]

        for o in origins:
            origin = self.get_node(o)
            for d in self.graph:
                destination = self.get_node(d)
                connectivity = origin.ping(destination)
                if connectivity:
                    print(f"{o}->{d} ✓     ", flush=True, end='\r')
                else:
                    all_connected = False
                    if o == d:
                        print(f"{o}->{d} (✗)", flush=True)
                    else:
                        print(f"{o}->{d} ✗", flush=True)

        return all_connected
