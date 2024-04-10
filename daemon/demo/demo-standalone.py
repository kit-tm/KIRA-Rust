#!/usr/bin/env python3
from docker.models.containers import Container
from docker.models.networks import Network
import docker
import networkx as nx
import os
import sys
import re

import argparse
import logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)


class R2KadNetwork:
    def __init__(self, graph: nx.Graph, client=docker.from_env()):
        self._networks = client.networks.list()
        self._logger = logging.getLogger(self.__class__.__name__)

        self.graph = graph
        self.client = client

        # init all nodes in graph
        for node in self.graph:
            self._init_node(node)

        for (u, v) in self.graph.edges:
            self._init_existing_network(u, v)

    def _init_node(self, node: int):
        self._logger.debug(f"Trying to find existing container for {node}")
        container_name = f"r2kad-n{node}"

        try:
            # find existing container
            container = self.client.containers.get(container_name)
            self._logger.debug(f"Adopting previous created container {
                               container.name} for node {node}")
            self.graph.nodes[node]["container"] = container
        except docker.errors.NotFound:
            pass

    def _init_existing_network(self, u, v):
        self._logger.debug(
            f"Trying to find existing network for edge {u}--{v}")
        # test if we already created the network in a previous run
        for nw in self._networks:
            if nw.name == f"r2edge-{u}-{v}":
                self._logger.debug(f"Adopting previous created network {
                                   nw.name} for edge {u}--{v}")
                self.graph.edges[u, v]["network"] = nw
                break

    def create_node_container(self, node: int, default_img: str, info=True) -> Container:
        container_name = f"r2kad-n{node}"

        img = self.graph.nodes[node].get("img", default_img)
        if img is None:
            logger.error(
                "Couldn't determine the image to use for the node container!")
            return None

        # create new container
        container = self.client.containers.create(
            image=img, name=container_name,
            detach=True,
            cap_add=["NET_ADMIN"],
            sysctls={
                'net.ipv6.conf.default.disable_ipv6': 0,
                'net.ipv6.conf.eth0.disable_ipv6': 1,
                'net.ipv6.conf.all.forwarding': 1
            },
            environment=[f"NODE_ID={int(node):028x}", f"RUST_LOG={os.environ.get('RUST_LOG', 'debug')}"])
        if info:
            self._logger.info(f"Created new container for node: {node}")

        self.graph.nodes[node]["container"] = container
        return container

    def create_edge_network(self, origin: int, destination: int) -> Network:
        network = self.client.networks.create(
            name=f"r2edge-{origin}-{destination}",
            driver="bridge",
            options={
                "com.docker.network.bridge.name": f"r2edge-{origin}-{destination}",
                "com.docker.network.container_iface_prefix": "r2edge"},
            enable_ipv6=True)
        self.graph.edges[origin, destination]["network"] = network
        return network

    def create(self, default_img: str) -> None:
        for (node, container) in self.graph.nodes(data="container"):
            if container is None:
                container = self.create_node_container(node, default_img)

    def start(self, img: str = None) -> None:
        for (node, container) in self.graph.nodes(data="container"):
            if container is None:
                if img is None:
                    self._logger.warning(
                        f"Couldn't start nor create container for node: {node}")
                    return
                container = self.create_node_container(node, img)
            container.start()
            self._logger.info(f"Started node {node}.")

    def stop(self) -> None:
        for (node, container) in self.graph.nodes(data="container"):
            if container is None:
                self._logger.warning(
                    f"Consider node without container stopped: {node}!")
                continue

            container.stop()
            self._logger.info(f"Stopped node {node}.")

    def connect(self, fallback_img: str = None) -> None:
        for (o, d, nw) in graph.edges.data(data="network"):
            self.connect_edge(o, d, network=nw, fallback_img=fallback_img)

    def connect_edge(self, origin: int, destination: int, network: Network = None, fallback_img: str = None) -> Network:
        if network is None:
            network = self.create_edge_network(origin, destination)
            self._logger.info(
                f"Created new network for the connection: {network.name}")

        # get containers of origin and destination or try to create them
        containers = {}
        for node in [origin, destination]:
            if "container" in self.graph.nodes[node]:
                containers[node] = self.graph.nodes[node]["container"]
            else:
                if fallback_img is None:
                    self._logger.error(f"Couldn't create edge {
                                       origin}--{destination}, since {node} has no container")
                containers[node] = self.create_node_container(
                    node, fallback_img, info=False)
                self._logger.info(f"Created node {node} to be able to connect {
                                  origin}--{destination}")

        # connect containers to network
        network.connect(container=containers[origin])
        network.connect(container=containers[destination])

        return network

    def prune(self) -> None:
        # self.stop()
        for (_, container) in self.graph.nodes(data="container"):
            if container is None:
                continue
            container.remove(v=True, force=True)
            self._logger.info(f"Deleted container {container.name}")

        for (_, _, nw) in graph.edges.data(data="network"):
            if nw is None:
                continue
            nw.remove()
            self._logger.info(f"Deleted network interface {nw.name}.")

    def store(self, node: int, reference: str, data: str) -> str:
        if "container" not in self.graph.nodes[str(node)]:
            self._logger.error(f"Can't store at {
                               node} without a container present!")
            return None
        container = self.graph.nodes[str(node)]["container"]

        cmd = f"curl localhost:8080/dht/store?reference={
            reference} -d \"{data}\""

        (_, out) = container.exec_run(cmd)
        return out

    def fetch(self, node: int, reference: str) -> str:
        if "container" not in self.graph.nodes[str(node)]:
            self._logger.error(f"Can't fetch at {
                               node} without a container present!")
            return None
        container = self.graph.nodes[str(node)]["container"]

        cmd = f"curl localhost:8080/dht/fetch?reference={reference}"

        (_, out) = container.exec_run(cmd)
        return out

    def logs(self, node: int, follow=False) -> str:
        if "container" not in self.graph.nodes[str(node)]:
            self._logger.error(f"Can't fetch logs at {
                               node} without a container present!")
            return None
        container = self.graph.nodes[str(node)]["container"]

        return container.logs()


if __name__ == '__main__':

    parser = argparse.ArgumentParser(description='r2kad topology generator')

    parser.add_argument('operation', type=str, choices=[
                        "create", "connect", "start", "stop", "prune", "store", "fetch", "logs"])
    parser.add_argument('-n', '--node-id', type=int, required=False, dest="node",
                        help="ID of the node to which to send store/fetch requests")
    parser.add_argument('-ref', '--reference', type=str, required=False, dest="reference",
                        help="Reference used by the store/fetch operation")
    parser.add_argument('-d', '--data', type=str, required=False, dest="data",
                        help="Data to store at given node")
    parser.add_argument('--edges', type=argparse.FileType("r"), required=True,
                        help="Path pointing to an edgelist")
    parser.add_argument('--img', type=str,
                        help="Docker image used by the node containers")
    args = parser.parse_args()

    edgefile = args.edges
    try:
        graph = nx.read_edgelist(edgefile)
    except FileNotFoundError:
        logger.error(f"Couldn't read edgelist file: {edgefile}!")
        sys.exit(202)

    try:
        network = R2KadNetwork(graph)

        operation = args.operation.lower()
        # this requires python>=3.10
        match operation:
            case "create":
                if args.img is None:
                    logger.error("No default image to use was specified!")
                    exit(101)
                network.create(args.img)
                logger.info("All nodes are created and ready to be started.")
            case "start":
                network.start(args.img)
                logger.info("All nodes are started.")
            case "stop":
                network.stop()
                logger.info("All nodes are stopped.")
            case "connect":
                network.connect(args.img)
                logger.info("All edges are connected.")
            case "prune":
                answer = input(
                    "Pruning might delete other containers not managed by this script. Do you really want to continue? y/N:  ")
                if not answer.strip().lower() == "y":
                    logger.info("Stopped pruning.")
                    sys.exit()
                network.prune()
                logger.info("Pruning done.")
            case "store":
                result = network.store(args.node, args.reference, args.data)
                result = result.decode("utf-8")
                print(result)
            case "fetch":
                result = network.fetch(args.node, args.reference)
                result = result.decode("utf-8")
                print(result)
            case "logs":
                result = network.logs(args.node)
                result = result.decode("utf-8")
                print(result)

            case _:
                logger.warn(f"Unknown operation: {operation}")
                sys.exit(2)
    except docker.errors.APIError as err:
        logger.exception(err)
        sys.exit(1)
