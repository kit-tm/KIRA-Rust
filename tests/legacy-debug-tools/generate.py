#!/usr/bin/env python3
import argparse
import networkx as nx
import random

from dataclasses import asdict
from common import NodeConfig

class TestConfigGenerator:
    def __init__(self, n=10, m=20, image: str = "kira", seed: int = 0):
        self.seed = seed
        self._rng = random.Random(seed)
        self.image = image
        self.n = n
        self.m = m

    def generate(self, failure):
        self.generate_topology()
        self.augment()
        if failure:
            self.failure_augment()
        return self.topology

    def generate_topology(self):
        self.topology = nx.gnm_random_graph(self.n, self.m, seed=self.seed)
        if not nx.is_connected(self.topology):
            self.topology.add_edges_from(nx.k_edge_augmentation(self.topology, k=1))


    def generate_id(self):
        ''' returns node id as hex and ipv6'''
        nid_bytes = self._rng.getrandbits(112).to_bytes(14, "big")
        return  (nid_bytes.hex(), "fc00:" + nid_bytes.hex(':', 2))

    def augment(self):
        for node in self.topology.nodes:
            nid, ipv6 = self.generate_id()
            self.topology.nodes[node]["config"] = asdict(NodeConfig(nid, ipv6, f"k{node}", self.image))

    def failure_augment(self):
        '''add a random link that fails, this way the topology will still be connected after the failure'''

        nodes = list(self.topology.nodes)
        a, b = self._rng.choices(nodes, k=2)
        while a == b or nx.shortest_path_length(self.topology, a, b) <= 1:
            a, b = self._rng.choices(nodes, k=2)

        self.topology.add_edge(a, b, fail=True)

def main(args):
    #generate
    G = TestConfigGenerator(args.n, args.m, args.image, args.seed).generate(args.failure)

    #write
    nx.readwrite.write_gml(G, f"{args.out_dir}/{args.name}.gml")

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description='Test Scenario Generation Script')
    parser.add_argument('name', type=str, help="The name of the test")
    parser.add_argument('--img', dest="image", type=str, default="kira", help="Docker image used by the node containers")
    parser.add_argument('--out-dir', dest="out_dir", type=str, default="./", help="Where to output the gml file")
    parser.add_argument('-n', type=int, default=10, help="number of nodes")
    parser.add_argument('-m', type=int, default=20, help="number of edges")
    parser.add_argument('--seed', type=int, default=0, help="seed for the rng")
    parser.add_argument('--failure', action='store_true', help="whether to add a failure event")
    args = parser.parse_args()


    main(args)
