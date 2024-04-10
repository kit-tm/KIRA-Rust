from mininet.net import Containernet
from mininet.node import Controller

import argparse
import networkx as nx
import sys
import os

import logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)


class R2KadNetwork:
    def __init__(self, graph: nx.Graph, net: Containernet):
        self._logger = logging.getLogger(self.__class__.__name__)

        self.graph = graph
        self._net = net

    def create_node_container(self, node: int, default_img: str):
        topo_name = self.graph.nodes[node].setdefault(
            "topo_name", f"r2kad-n{node}")

        img = self.graph.nodes[node].get("img", default_img)
        if img is None:
            logger.error(
                "Couldn't determine the image to use for the node container!")
            return None

        sysctls = {
            'net.ipv6.conf.default.disable_ipv6': 0,
            'net.ipv6.conf.all.forwarding': 1,
        }
        environment = [
            f"NODE_ID={int(node):028x}",
            f"RUST_LOG={os.environ.get('RUST_LOG', 'debug')}"
        ]
        cmd = "/usr/bin/supervisord -c /etc/supervisord.conf"
        container = self._net.addDocker(topo_name, ip=None, dimage=img,
                                        cap_add=["NET_ADMIN"],
                                        sysctls=sysctls,
                                        environment=environment,
                                        dcmd=cmd,
                                        dns=["127.0.0.1"])

        self.graph.nodes[node]["container"] = container
        self.graph.nodes[node]["topo_name"] = topo_name
        return container

    def create_edge_network(self, origin: int, destination: int):
        origin_name = self.graph.nodes[origin]["topo_name"]
        destination_name = self.graph.nodes[destination]["topo_name"]
        network = self._net.addLink(origin_name, destination_name)

        self.graph.edges[origin, destination]["network"] = network
        return network

    def start(self, default_img: str):
        for u in self.graph:
            self.create_node_container(u, default_img)

        for u, v in self.graph.edges:
            self.create_edge_network(u, v)

        self._logger.info("Starting Containernet")
        self._net.start()


if __name__ == '__main__':

    parser = argparse.ArgumentParser(
        description='r2kad topology generator using Containernet')

    parser.add_argument('--edges', type=argparse.FileType("r"),
                        required=True, help="Path to an edgelist")
    parser.add_argument('--img', type=str,
                        help="Docker image used by the node containers")
    args = parser.parse_args()

    default_img = args.img
    edge_file = args.edges
    try:
        graph = nx.read_edgelist(edge_file)
    except FileNotFoundError:
        logger.error(
            f"Couldn't read edgelist file: {edge_file}!")
        sys.exit(202)

    net = Containernet(controller=Controller)
    kad_net = R2KadNetwork(graph, net)
    kad_net.start(default_img)
