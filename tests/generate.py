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

    def generate(self):
        self.generate_topology()
        self.augment()
        return self.topology

    def generate_topology(self):
        self.topology = nx.gnm_random_graph(self.n, self.m, seed=self.seed)

    def generate_id(self):
        ''' returns node id as hex and ipv6'''
        nid_bytes = self._rng.getrandbits(112).to_bytes(14, "big")
        return  (nid_bytes.hex(), "fc00:" + nid_bytes.hex(':', 2))

    def augment(self):
        for node in self.topology.nodes:
            nid, ipv6 = self.generate_id()
            self.topology.nodes[node]["config"] = asdict(NodeConfig(nid, ipv6, f"k{node}", self.image))

def main(args):
    #generate
    G = TestConfigGenerator(args.n, args.m, args.image, args.seed).generate()

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
    args = parser.parse_args()
    

    main(args)
