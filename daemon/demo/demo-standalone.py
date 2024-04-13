#!/usr/bin/env python3
from docker.models.containers import Container
from docker.models.networks import Network
import docker
import networkx as nx
import os
import sys
import random
import re
from typing import Optional

import argparse
import logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)


class R2KadNetwork:
    def __init__(self, graph: nx.Graph, client=docker.from_env(), seed=None):
        self._networks = client.networks.list()
        self._logger = logging.getLogger(self.__class__.__name__)

        if seed is None:
            seed = random.SystemRandom().randint(0, 1024)
            self._logger.info(f"Using Seed {seed} to generate ids of nodes")
        self._rng = random.Random(seed)

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

            # get node id
            nid = self.get_node_id(node)
            if nid is not None:
                self.graph.nodes[node]["node-id"] = nid
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

        ndata = self.graph.nodes[node]
        img = ndata.get("img", default_img)
        if img is None:
            logger.error(
                "Couldn't determine the image to use for the node container!")
            return None

        nid = ndata.get("node-id", self._rng.getrandbits(112))
        self._logger.info(f"Using node-id {int(nid):028x} for node {node}")

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
            environment=[f"NODE_ID={int(nid):028x}", f"RUST_LOG={os.environ.get('RUST_LOG', 'debug')}"])
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

    def start(self, default_img: str = None) -> None:
        for (node, container) in self.graph.nodes(data="container"):
            if container is None:
                if default_img is None:
                    self._logger.warning(
                        f"Couldn't start nor create container for node: {node}")
                    return
                container = self.create_node_container(node, default_img)
            container.start()
            self._logger.info(f"Started node {node}.")

    def stop(self) -> None:
        for (node, container) in self.graph.nodes(data="container"):
            if container is None:
                continue
            container.stop()
            self._logger.info(f"Stopped node {node}.")

    def connect(self) -> None:
        for (o, d, nw) in graph.edges.data(data="network"):
            self.connect_edge(o, d, network=nw)

    def connect_edge(self, origin: int, destination: int, network: Network = None) -> Optional[Network]:
        # get containers of origin and destination
        containers = {}
        for node in [origin, destination]:
            if "container" in self.graph.nodes[node]:
                containers[node] = self.graph.nodes[node]["container"]
            else:
                self._logger.error(f"Couldn't connect edge {
                                   origin}--{destination}: Node {node} has no container")
                return None

        if network is None:
            network = self.create_edge_network(origin, destination)
            self._logger.info(
                f"Created new network for the connection: {network.name}")

        # connect containers to network
        network.connect(container=containers[origin])
        network.connect(container=containers[destination])

        return network

    def prune(self) -> None:
        for (node, container) in self.graph.nodes(data="container"):
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

    def get_node_id(self, node: int) -> Optional[bytes]:
        if "container" not in self.graph.nodes[str(node)]:
            return None
        container = self.graph.nodes[str(node)]["container"]

        cmd = "curl localhost:8080/node-id"

        try:
            (_, out) = container.exec_run(cmd)
        except docker.errors.APIError:
            self._logger.debug(f"Can't exec in container of node {node}")
            return None

        out = out.decode("utf-8")
        match = re.search(r'\{"node-id"*+:*+"(.+)"\}', out)
        if match is None:
            self._logger.warning(f"Can't find node id in output: {out}")
            return None

        out = match.group(1)
        return bytes.fromhex(out)

    def ping(self, origin: int, destination: int) -> Optional[bool]:
        if "container" not in self.graph.nodes[str(origin)]:
            self._logger.error(f"Can't ping from node {
                               origin} without a container")
            return None
        container = self.graph.nodes[str(origin)]["container"]

        nid = self.graph.nodes[str(destination)].get(
            "node-id", self.get_node_id(destination))
        if nid is None:
            self._logger.error(
                f"Can't determine node-id of node {destination}")
            return None

        nid = nid.hex(":", 2)

        cmd = f"ping -c 3 -i 0.25 -W 1 -q fc00:{nid}"

        (res, _) = container.exec_run(cmd)
        return res == 0

    def test_connectivity(self):
        for o in self.graph:
            for d in self.graph:
                connectivity = self.ping(o, d)
                if connectivity:
                    print(f"{o}->{d} ✓", flush=True, end='\r')
                else:
                    print(f"{o}->{d} ✗", flush=True)

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
                        "create", "connect", "start", "stop", "prune", "store", "fetch", "logs", "node-id", "test_connectivity"])
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
    parser.add_argument('--seed', type=str, required=False,
                        help="Seed used to generate Node-IDs")
    args = parser.parse_args()

    edgefile = args.edges
    try:
        graph = nx.read_edgelist(edgefile)
    except FileNotFoundError:
        logger.error(f"Couldn't read edgelist file: {edgefile}!")
        sys.exit(202)

    try:
        network = R2KadNetwork(graph, seed=args.seed)

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
                network.connect()
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
            case "node-id":
                result = network.get_node_id(args.node)
                result = result.hex()
                print(result)
            case "test_connectivity":
                network.test_connectivity()
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
