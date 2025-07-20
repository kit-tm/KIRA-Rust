import argparse
import base64
import io
import json
import os
import re
import sys
from cmd import Cmd
from dataclasses import dataclass
from hashlib import sha1
from ipaddress import (
    AddressValueError,
    IPv4Address,
    IPv4Network,
    IPv6Address,
    IPv6Network,
)
from itertools import chain
from subprocess import PIPE, Popen
from typing import Any, Iterable, Iterator, Union

import matplotlib.pyplot as plt
import nest
import networkx as nx
import networkx
from common import NodeConfig
from nest.topology import Address, Interface, Node, Switch, connect
from nest.topology.interface.interface import create_veth_pair
from networkx import Graph
from term_image.image import AutoImage, BaseImage
from PIL import Image

PATH_IP: IPv6Network = IPv6Network("fcaa::/16")
NODE_IP: IPv6Network = IPv6Network("fc00::/16")

VICINITY_RADIUS: int = 2


class KIRANode(Node):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)

        self._api_port = 8080

    def exec(self, cmd: str, env_vars: dict | None = None, logfile=None) -> Popen:
        """
        Execute a command in the node's namespace.
        """
        if env_vars is None:
            env_vars = os.environ.copy()
        if logfile is None:
            print("No logfile provided, using stdout")
        return Popen(
            f"ip netns exec {self.id} {cmd}",
            shell=True,
            env=env_vars,
            stdout=logfile,
            stderr=logfile,
        )

    def api_call(self, path: str, payload: str | None = None) -> str | None:
        cmd = f"curl localhost:{self._api_port}/{path}"
        if payload:
            cmd += f" -d {payload}"

        p = self.exec(cmd, logfile=PIPE)
        stdout, _ = p.communicate()
        return stdout.decode("utf-8") if p.returncode == 0 else None

    def store(self, key: str, data: str) -> str | None:
        path = f"dht/store?key={key}"
        return self.api_call(path, data)

    def fetch(self, key: str) -> list[str]:
        path = f"dht/fetch?key={key}"
        res = self.api_call(path)
        if res is None:
            return []

        # decode
        try:
            res = json.loads(res)
        except json.decoder.JSONDecodeError:
            return []
        return [base64.b64decode(value).decode("utf-8") for value in res]

    def routing_table(self) -> str | None:
        path = "_dev/routing-table"
        return self.api_call(path)

    def uln_table(self) -> str | None:
        path = "_dev/uln-table"
        return self.api_call(path)

    def vicinity_graph(self) -> str | None:
        path = "_dev/vicinity-graph"
        return self.api_call(path)

    def local_hashtable(self) -> str | None:
        path = "dht/_dev/local-hashtable"
        return self.api_call(path)

    def root_id(self) -> bytes | None:
        path = "node-id"
        res = self.api_call(path)
        if res is None:
            return None
        res = json.loads(res)
        return bytes.fromhex(res.get("node-id"))

    def node_id(self) -> bytes | None:
        return self.root_id()

    def is_up(self) -> bool:
        path = "node-id"
        res = self.api_call(path)
        if res is None:
            return False
        res = json.loads(res)
        return "node-id" in res

    def ping(
        self,
        destination_address: Address,
        preload: int = 1,
        packets: int = 5,
        verbose: int = 2,
        timeout: int = 1,
    ):
        # overwrite ping to support timeout
        dst_addr = destination_address.get_addr(with_subnet=False)
        if verbose not in [0, 1, 2]:
            raise ValueError(f"Verbose parameter value is {verbose}. It should be 0, 1 or 2.")

        if verbose == 2:
            print()
            print(
                f"=== PING from {self.name} to "
                f"{destination_address.get_addr(with_subnet=False)} ==="
            )
            print()

        p = self.exec(
            f"ping -l {preload} -c {packets} -W {timeout} {dst_addr}",
            logfile=sys.stdout if verbose else PIPE,
        )
        status = p.wait() == 0

        if verbose == 1:
            print()
            if status is True:
                print(f"SUCCESS : === PING from {self.name} to {dst_addr} ===")
            elif status is False:
                print(f"FAILURE: === PING from {self.name} to {dst_addr} ===")
            print()

        return status

    def next_ip(self, ip: IPv6Address) -> IPv6Address | None:
        """
        Returns next `IPv6Address` used for forwarding of the packet.
        Usually this IP changes per hop but can remain the same,
        if the packet is not encapsulated but forwarded unchanged to the next hop.
        """

        if ip in PATH_IP:
            # lookup nftables forwardmap for translation

            cmd = f"nft get element ip6 kira forwardmap {{ {ip} }}"
            p = self.exec(cmd, logfile=PIPE)
            stdout, _ = p.communicate()
            if p.returncode != 0:
                # print(f"IPv6Address {ip} unknown to node {self}")
                return None
            stdout = stdout.decode("utf-8")

            # parse output
            match = re.search(r"elements = { [a-fA-F0-9:]+ : ([a-fA-F0-9:]+) }", stdout)
            if match:
                next_ip = IPv6Address(match.group(1))
                # label swap or decap
                assert next_ip in PATH_IP or next_ip in NODE_IP
                return next_ip
            else:
                # print(f"IPv6Address {ip} unknown to node {self}, parsing failed.")
                return None
        elif ip in NODE_IP:
            # lookup ip route
            cmd = f"ip -j route get {ip}"
            p = self.exec(cmd, logfile=PIPE)
            stdout, _ = p.communicate()
            if p.returncode != 0:
                # print(f"IPv6Address {ip} unknown to node {self}")
                return None
            stdout = stdout.decode("utf-8")

            # parse output
            result = json.loads(stdout)
            assert len(result) == 1
            result = result[0]

            # OPTION 1: ENCAP
            if "encap" in result:
                try:
                    path_ip = IPv6Address(result["encap"]["dst"])
                except TypeError as e:
                    # See https://git.kernel.org/pub/scm/network/iproute2/iproute2.git/commit/?id=0f32ef97babcbe77140a69218917937e6a50fb6c
                    raise RuntimeError(
                        "iproute2 version >= 6.9.0 required for JSON output fix"
                    ) from e
                assert path_ip in PATH_IP
                return path_ip

            # OPTION 2: it's us
            if result.get("type") == "local":
                # IP technically doesn't change
                return ip

            # OPTION 3: physical neighbor
            if "dst" in result and ip in IPv6Network(result["dst"]):
                return ip
            # print(f"Unexpected route for {ip} on node {self}: {result}")
            return None
        else:
            # print(f"Unexpected IPv6Address {ip} is neither Path- nor Node-IP")
            return None

    def next_hop(self, ip: IPv6Address) -> IPv6Address | None:
        """
        Returns IPv6Address (Node-IP) of the next hop the packet will go to.
        The supplied IPv6Address is the egress IPv6Address of the packet,
        so after label switching, encapsulation, ... .

        To determine this next ip use `self.next_ip`.
        """
        # lookup ip route
        cmd = f"ip -6 -j route get {ip}"
        p = self.exec(cmd, logfile=PIPE)
        stdout, _ = p.communicate()
        if p.returncode != 0:
            # print(f"IPv6Address {ip} unknown to node {self}")
            return None
        stdout = stdout.decode("utf-8")

        # parse output
        result = json.loads(stdout)
        assert len(result) == 1
        result = result[0]

        # OPTION 1: it's us
        if result.get("type") == "local":
            assert IPv6Address(result["prefsrc"]) == ip
            return ip

        # OPTION 2: Forward VIA next hop
        if "gateway" in result:  # gateway == via :/
            gateway = IPv6Address(result["gateway"])
            if gateway in NODE_IP:
                return gateway
            # in newer implementation we use LL-IPv6 as gateway
            assert gateway.is_link_local
            dev = result.get("dev")

            cmd = "ip -6 -j route show"
            p = self.exec(cmd, logfile=PIPE)
            stdout, _ = p.communicate()
            if p.returncode != 0:
                # print(f"IPv6Address {ip} unknown to node {self}")
                return None
            # parse output
            stdout = stdout.decode("utf-8")
            routes = json.loads(stdout)

            # find NODE_IP dev <dev> entry to obtain NODE_IP
            next_hop = None
            for route in routes:
                if route.get("dev") == dev and route.get("protocol") == "static":
                    try:
                        dst = IPv6Address(route.get("dst"))
                        if dst in NODE_IP:
                            # HACK: this only works in unswitched networks
                            #       where one node is reachable per interface
                            assert next_hop is None
                            next_hop = dst
                    except AddressValueError:
                        continue
            return next_hop

        # OPTION 3: Overlay Hop -- (re)encapsulation on this node required
        if "encap" in result:
            assert ip in NODE_IP
            # determine own ip
            if "prefsrc" in result:
                node_ip = IPv6Address(result["prefsrc"])
                assert node_ip in NODE_IP
                return node_ip
            else:
                return None
        # OPTION 4: Physical neighbor
        if "dst" in result and IPv6Address(result["dst"]) == ip:
            assert ip in NODE_IP
            return ip

        # print(f"Unexpected route for {ip} on node {self}: {result}")
        return None

    def paths_rt(self) -> Iterator[Iterator[bytes]] | None:
        """Returns an iterator over all paths (node-id sequence) known by the node."""
        # FIXME:Consider Vicinity Paths
        routing_table = self.routing_table()
        if routing_table is None:
            return None
        # join lines

        # find all paths
        r_path = re.compile(r"Path\s+{\s+ids:\s+\[([\s\w(:),]+)\]", flags=re.MULTILINE)
        r_nid = re.compile(r"NodeId\(([\w]+)\),")

        for path_match in r_path.finditer(routing_table):
            path_str = path_match.group(1)
            hops = (bytes.fromhex(nid_match.group(1)) for nid_match in r_nid.finditer(path_str))
            yield hops

    def paths_vicinity(self) -> Iterator[list[bytes]] | None:
        # parsing vicinity graph of Node
        root_id = self.root_id()
        vicinity_raw = self.vicinity_graph()
        if vicinity_raw is None or root_id is None:
            return None
        node_id_re = re.compile(r"NodeId\((\w+)\)")

        # Parse neighbor relationships
        vicinity_graph = Graph()
        entry_re = re.compile(
            r"NodeId\((\w+)\): Entry \{[^}]*?neighbors: \{([^}]*)\}",
            re.DOTALL,  # match newline
        )
        for match in entry_re.finditer(vicinity_raw):
            vid = bytes.fromhex(match.group(1))
            neighbors = match.group(2)
            neighbor_ids = node_id_re.findall(neighbors)

            for uid in neighbor_ids:
                uid = bytes.fromhex(uid)
                vicinity_graph.add_edge(vid, uid)

        for v in vicinity_graph.nodes:
            yield from nx.all_simple_paths(vicinity_graph, root_id, v, cutoff=VICINITY_RADIUS)

    @staticmethod
    def path_id(path: Iterable[bytes]) -> bytes:
        """Calculate the Path-ID from a path of NodeIds."""
        path_id = sha1(b"".join(path)).digest()[:14]
        return path_id

    def path(self, ip: IPv6Address) -> list[bytes] | None:
        """Translate a Path-IP address (fcaa::/16) into
        its corresponding path of Node-IPs.
        """
        assert ip in PATH_IP

        # usually a path used is in some routing table
        # but just to be sure we check the vicinity too
        paths_rt = self.paths_rt()
        if paths_rt is None:
            return None
        paths_vicinity = self.paths_vicinity()
        if paths_vicinity is None:
            return None
        paths = chain(paths_rt, paths_vicinity)

        for path in paths:
            path = list(path)
            path_id = KIRANode.path_id(path)
            path_ip = IPv6Address(bytes.fromhex("fcaa") + path_id)
            if ip == path_ip:
                return path

    def __format__(self, fmt):
        return f"{self.name:{fmt}}"

    def __str__(self):
        return self.__format__("")

    def __eq__(self, other):
        return other is KIRANode and self.name == other.name

    def __hash__(self):
        return hash(self.name)


class KIRALink:
    _is_up: bool
    _interface_x: Interface
    _interface_y: Interface

    def __init__(self, inteface_x: Interface, interface_y: Interface):
        self._interface_x = inteface_x
        self._interface_y = interface_y

        # just to be sure
        self.up()

    def down(self) -> None:
        self._is_up = False
        self._interface_x.set_mode("DOWN")
        self._interface_y.set_mode("DOWN")

    def up(self) -> None:
        self._is_up = True
        self._interface_x.set_mode("UP")
        self._interface_y.set_mode("UP")

    def is_up(self) -> bool:
        return self._is_up


@dataclass
class NodeIdEncapEntry:
    node: KIRANode
    path: list[KIRANode]
    path_id: bytes


@dataclass
class UnderlayNeighborFwdEntry:
    node: KIRANode


@dataclass
class PathIdFwdEntry:
    path: list[KIRANode]
    path_id: bytes


@dataclass
class PathIdEncapEntry:
    in_path: list[KIRANode]
    in_path_id: bytes
    out_path: list[KIRANode]
    out_path_id: bytes


FwdEntry = Union[NodeIdEncapEntry, UnderlayNeighborFwdEntry, PathIdFwdEntry, PathIdEncapEntry]


class NestTest[T]:  # T = tid type, usually int or str
    _otel_ip: IPv4Network = IPv4Network("10.42.0.0/24")

    """
    Nest Test
    ===========

    This is a test for the Nest framework. It creates a topology and runs a connectivity test between nodes.

    Parameters
    ----------
    config : dict
        The configuration for the test.
    """

    def __init__(self, config: Graph, otel_ip: IPv4Network | None = None):
        if otel_ip is not None:
            self._otel_ip: IPv4Network = otel_ip

        self.topology: Graph = config
        self.name_tid_mapping: dict[str, T] = {}
        self._otel_node: Node | None = None  # lazily initialized as needed
        self._otel_ips: Iterable[IPv4Address] = self._otel_ip.hosts()
        next(self._otel_ips)  # skip IP for host

        self._pos = networkx.kamada_kawai_layout(self.topology)

        nest.logging.info("Setting up the topology ...")

        # Create the Nest topology according to the configuration
        for tid, cfg in self.topology.nodes(data="config"):
            assert type(cfg) is NodeConfig
            name = cfg.name
            node = KIRANode(name)
            node.enable_ip_forwarding(True, True)
            self.topology.nodes[tid]["node"] = node
            self.name_tid_mapping[name] = tid

        nest.logging.info("Setting up interfaces ...")
        for x, y in self.topology.edges:
            nx = self.topology.nodes[x]["node"]
            ny = self.topology.nodes[y]["node"]

            if_x, if_y = connect(nx, ny, f"n{x}n{y}", f"n{y}n{x}")
            if_x.set_address(self.topology.nodes[x]["config"].ipv6)
            if_y.set_address(self.topology.nodes[y]["config"].ipv6)

            # safe interfaces for later
            self.topology.edges[x, y]["link"] = KIRALink(if_x, if_y)

        nest.logging.info("Starting daemons ...")
        for node, cfg in self.nodes():
            node_id = cfg.node_id
            args: list[str] = []

            if cfg.otel:
                args.append("--open-telemetry")
                otel = self.otel_node
                if_otel, _ = connect(node, otel, f"{node}notel", f"oteln{node}")

                # disable automatic IPv6 address
                # don't gain KIRA connectivity via switch
                Popen(
                    [
                        "ip",
                        "netns",
                        "exec",
                        if_otel.node_id,
                        "ip",
                        "link",
                        "set",
                        "dev",
                        if_otel.id,
                        "addrgenmode",
                        "none",
                    ]
                )
                if_otel.set_mode("DOWN")
                if_otel.set_mode("UP")

                node_otel_ip = next(self._otel_ips)
                assert node_otel_ip is not None, (
                    f"Run out of IPs in {self._otel_ip} to assign to OTel interfaces"
                )
                if_otel.set_address(f"{node_otel_ip.exploded}/{self._otel_ip.prefixlen}")
                Popen(
                    [
                        "ip",
                        "netns",
                        "exec",
                        if_otel.node_id,
                        "ip",
                        "route",
                        "add",
                        "default",
                        "dev",
                        if_otel.id,
                    ]
                )

            logfile = f"{node}.log"
            env_vars = os.environ.copy()
            env_vars["RUST_LOG_STYLE"] = "never"
            env_vars["NO_COLOR"] = "1"
            env_vars["RUST_LOG"] = env_vars.get("RUST_LOG", "info")
            env_vars["RUST_BACKTRACE"] = "1"
            arg: str = " ".join(args)
            with open(logfile, "w") as f:
                node.exec(
                    f"./target/debug/kirad --root-id {node_id} --nftables-conf ./kirad/conf/nftables.conf {arg} && exit",
                    logfile=f,
                    env_vars=env_vars,
                )

    @property
    def otel_node(self):
        if self._otel_node is None:
            if all(
                env not in os.environ
                for env in [
                    "OTEL_EXPORTER_OTLP_ENDPOINT",
                    "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
                ]
            ):
                print(
                    "WARN: You haven't set any OTEL_EXPORTER_OTLP*_ENDPOINT environment variable. OpenTelemetry probably won't work."
                )

            self._otel_node = Switch("OpenTelemetry Collector")
            assert self._otel_node is not None
            host, otel = create_veth_pair("otel", "otels")

            # move otel interface to otel_node
            self._otel_node._add_interface(otel)
            otel.set_mode("UP")

            # but leave host interface in default ns for connection
            # since it's not connected to a node we need to invoke `ip` ourselves
            host_ip = f"{next(self._otel_ip.hosts()).exploded}/{self._otel_ip.prefixlen}"
            host = host.id

            Popen(["ip", "address", "add", "dev", host, host_ip])
            Popen(["ip", "link", "set", "dev", host, "up"])

        return self._otel_node

    def tid(self, node: KIRANode) -> T | None:
        name = node.name
        return self.name_tid_mapping.get(name)

    def node(self, tid: T) -> KIRANode:
        return self.topology.nodes[tid]["node"]

    def node_by_ip(self, ip: IPv6Address) -> KIRANode | None:
        assert ip in NODE_IP
        for _, data in self.topology.nodes(data=True):
            config = data["config"]
            nip = IPv6Address(config.ipv6)
            if nip == ip:
                return data["node"]

    def node_by_id(self, id: bytes) -> KIRANode | None:
        for _, data in self.topology.nodes(data=True):
            config: NodeConfig = data["config"]
            nid = bytes.fromhex(config.node_id)
            if nid == id:
                return data["node"]

    def nodes(self) -> Iterator[tuple[KIRANode, NodeConfig]]:
        return ((ndata["node"], ndata["config"]) for _, ndata in self.topology.nodes(data=True))

    def node_id(self, tid: T) -> bytes:
        config: NodeConfig = self.topology.nodes[tid]["config"]
        return bytes.fromhex(config.node_id)

    def link(self, x_tid: T, y_tid: T) -> KIRALink:
        return self.topology.edges[x_tid, y_tid]["link"]

    def links(self, of: T | None) -> Iterator[tuple[T, T, KIRALink]]:
        return (t for t in self.topology.edges(of, data="link"))

    def _describe_hop_action(
        self,
        current_hop: KIRANode,
        from_addr: IPv6Address,
        to_addr: IPv6Address,
        path: list[bytes] | None,
    ) -> str:
        assert from_addr in PATH_IP or to_addr in PATH_IP

        if path is None:
            path = "???"
        else:
            # transform path from seq(nid) into seq(tid)
            path = (IPv6Address(bytes.fromhex("fc00") + hop) for hop in path)
            path = ", ".join((str(self.node_by_ip(hop)) for hop in path))

        # print action that lead to label change
        if from_addr in PATH_IP and to_addr in PATH_IP:
            return f"{current_hop:<3} : SWAP Path-ID : {from_addr} --> {to_addr} ({path})"
        elif from_addr in PATH_IP:
            return f"{current_hop:<3} : POP  Path-ID : {from_addr} --> {to_addr}"
        elif to_addr in PATH_IP:
            return f"{current_hop:<3} : PUSH Path-ID : {to_addr} ({path})"
        else:
            return "??? Unknown Action ???"

    def traceroute(self, x_tid: T, y_tid: T, maxhops: int = 50, verbose: bool = False) -> bool:
        current_hop = self.node(x_tid)
        current_ip = IPv6Address(self.topology.nodes[x_tid]["config"].ipv6)

        dst_hop = self.node(y_tid)
        dst_ip = IPv6Address(self.topology.nodes[y_tid]["config"].ipv6)
        outer_ip = dst_ip
        current_path = None

        if verbose:
            print(f"Tracerouting from {current_hop} to {dst_ip} ({dst_hop}):")

        hc = 0
        while hc <= maxhops:
            prev_outer_ip = outer_ip
            outer_ip = current_hop.next_ip(outer_ip)
            if outer_ip is None:
                if verbose:
                    print(f"{current_hop:<3} : ERR unknown  : {prev_outer_ip}")
                return False

            # label change
            if prev_outer_ip != outer_ip:
                # pop label
                # done automagically by nftables
                if outer_ip == current_ip:
                    outer_ip = dst_ip

                # lookup corresponding path on node
                # that rerouted the packet (overlay hop)
                if prev_outer_ip in NODE_IP and outer_ip in PATH_IP:
                    current_path = current_hop.path(outer_ip)
                    current_path = None if current_path is None else list(current_path)
                elif current_path is not None:
                    current_path = current_path[1:]

                # don't change intended destination!
                assert prev_outer_ip in PATH_IP or outer_ip in PATH_IP

                # print action that lead to label change
                if verbose:
                    descr = self._describe_hop_action(
                        current_hop, prev_outer_ip, outer_ip, current_path
                    )
                    print(descr)

            next_ip = current_hop.next_hop(outer_ip)
            if next_ip is None:
                print(f"{current_hop:<3} : ERR : Can't determine next ip")
                return False
            next_hop = self.node_by_ip(next_ip)
            if next_hop is None:
                print(f"{current_hop:<3} : ERR : Can't determine next hop based on IPv6: {next_ip}")
                return False

            if current_hop != next_hop:
                hc += 1
                if verbose:
                    print(f"{current_hop:<3} : FORWARD to {next_hop}")

            current_hop = next_hop
            current_ip = next_ip

            # packet reached destination hop unencapsulated
            if current_ip == dst_ip and outer_ip == dst_ip:
                if verbose:
                    print(f"{current_hop:<3} : ACK")
                return True

        # maxhop limit reached
        if verbose:
            print(f"{current_hop:<3} : HLIMIT = {maxhops} reached")
        return False

    def vicinity_hc(self, node: KIRANode) -> dict[KIRANode, int]:
        tid = self.name_tid_mapping[node.name]
        paths = nx.single_source_shortest_path(
            self.topology,
            tid,
            # asking for neighbors inside of nodes _inside_ the vicinity
            # but not adding them to vicinity graph if on the edge
            cutoff=VICINITY_RADIUS,
        )
        return {
            self.node(n): len(path) - 1  # hopcount excludes ourselves
            for n, path in paths.items()
            if self.node(n) is not None and len(path) > 1
        }

    def vicinity(self, node: KIRANode) -> Iterator[KIRANode]:
        return self.vicinity_hc(node).keys().__iter__()

    def vicinity_edges(self, node: KIRANode) -> Iterator[tuple[KIRANode, KIRANode]]:
        vicinity = self.vicinity_hc(node)
        vicinity[node] = 0

        for u, v in self.topology.edges():
            u = self.node(u)
            v = self.node(v)
            uhc = vicinity.get(u)
            vhc = vicinity.get(v)
            if uhc is None or vhc is None:
                continue
            assert uhc <= VICINITY_RADIUS
            assert vhc <= VICINITY_RADIUS

            # no links between edge nodes
            if uhc == VICINITY_RADIUS and vhc == VICINITY_RADIUS:
                continue

            yield (u, v)

    def discovered_vicinity(self, node: KIRANode) -> Iterator[KIRANode] | None:
        # parsing vicinity graph of Node
        vicinity_raw = node.vicinity_graph()
        if vicinity_raw is None:
            return None

        entry_re = re.compile(r"NodeId\((\w+)\): Entry \{")

        for match in entry_re.findall(vicinity_raw):
            nid = bytes.fromhex(match)
            nip = IPv6Address(bytes.fromhex("fc00") + nid)
            n = self.node_by_ip(nip)
            if n is None:
                print(f"WAR: Node-IP in vicinity of {self} unknown: {nip}")
                continue
            yield n

    def known_vicinity_edges(self, node: KIRANode) -> Iterator[tuple[KIRANode, KIRANode]] | None:
        # parsing vicinity graph of Node
        vicinity_raw = node.vicinity_graph()
        if vicinity_raw is None:
            return None
        node_id_re = re.compile(r"NodeId\((\w+)\)")

        # Parse neighbor relationships
        entry_re = re.compile(
            r"NodeId\((\w+)\): Entry \{[^}]*?neighbors: \{([^}]*)\}",
            re.DOTALL,  # match newline
        )
        for match in entry_re.finditer(vicinity_raw):
            vid = bytes.fromhex(match.group(1))
            vip = IPv6Address(bytes.fromhex("fc00") + vid)
            v = self.node_by_ip(vip)
            if v is None:
                print(f"WAR: Node-IP in vicinity of {self} unknown: {vip}")
                continue
            neighbors = match.group(2)
            neighbor_ids = node_id_re.findall(neighbors)

            for uid in neighbor_ids:
                uid = bytes.fromhex(uid)
                uip = IPv6Address(bytes.fromhex("fc00") + uid)
                u = self.node_by_ip(uip)
                if u is None:
                    print(f"WAR: Node-IP in vicinity of {self} unknown: {uip}")
                    continue

                yield (v, u)

    def additional_vicinity(self, node: KIRANode) -> Iterator[KIRANode] | None:
        vicinity = self.vicinity(node)
        kvicinity = self.discovered_vicinity(node)
        if kvicinity is None or vicinity is None:
            return None
        vicinity = set(vicinity)

        for known in kvicinity:
            if known not in vicinity:
                yield known

    def additional_vicinity_edges(
        self, node: KIRANode
    ) -> Iterator[tuple[KIRANode, KIRANode]] | None:
        vicinity = self.vicinity_edges(node)
        kvicinity = self.known_vicinity_edges(node)
        if kvicinity is None:
            return None

        for u, v in kvicinity:
            if (u, v) not in vicinity and (v, u) not in vicinity:
                yield (u, v)

    def unknown_vicinity(self, node: KIRANode) -> Iterator[KIRANode] | None:
        vicinity = self.vicinity(node)
        kvicinity = self.discovered_vicinity(node)
        if kvicinity is None or vicinity is None:
            return None
        kvicinity = set(kvicinity)

        for n in vicinity:
            if n not in kvicinity:
                yield n

    def unknown_vicinity_edges(self, node: KIRANode) -> Iterator[tuple[KIRANode, KIRANode]] | None:
        vicinity = self.vicinity_edges(node)
        kvicinity = self.known_vicinity_edges(node)
        if kvicinity is None:
            return None

        kvicinity = list(kvicinity)
        for u, v in vicinity:
            if (u, v) not in kvicinity and (v, u) not in kvicinity:
                yield (u, v)

    def vicinity_paths(self, node: KIRANode) -> Iterator[list[T]] | None:
        n = self.tid(node)
        assert n is not None

        for v in self.vicinity(node):
            v = self.tid(v)
            assert v is not None

            paths = nx.all_simple_paths(self.topology, n, v, cutoff=VICINITY_RADIUS)
            yield from paths

    def missing_pathsetups(self, node: KIRANode) -> Iterator[FwdEntry] | None:
        vicinity_paths = self.vicinity_paths(node)
        if vicinity_paths is None:
            return None
        # WARN: adjust if node.paths() gets changed

        for path in vicinity_paths:
            in_path = path
            out_path = path[1:]

            path = [self.node_id(tid) for tid in path]
            in_nid_path = path
            out_nid_path = path[1:]

            in_path_id = KIRANode.path_id(in_nid_path)
            out_path_id = KIRANode.path_id(out_nid_path)

            in_path_ip = IPv6Address(bytes.fromhex("fcaa") + in_path_id)
            out_path_ip = IPv6Address(bytes.fromhex("fcaa") + out_path_id)

            out_path = [self.node(tid) for tid in out_path]
            if node.next_ip(in_path_ip) is None:
                in_path = [self.node(tid) for tid in in_path]
                yield PathIdEncapEntry(in_path, in_path_id, out_path, out_path_id)
            if node.next_hop(out_path_ip) is None and len(out_path) > 1:
                yield PathIdFwdEntry(out_path, out_path_id)

    def missing_pathsetups_rt(self, node: KIRANode) -> Iterator[FwdEntry] | None:
        rt_paths = node.paths_rt()
        if rt_paths is None:
            return None

        for path in rt_paths:
            path = list(path)
            if len(path) == 0:
                continue

            dest = path[-1]
            ulneighbor = len(path) == 1
            dest_ip = IPv6Address(bytes.fromhex("fc00") + dest)
            next_ip = node.next_ip(dest_ip)

            out_path = path
            out_path_id = KIRANode.path_id(out_path)
            out_path_ip = IPv6Address(bytes.fromhex("fcaa") + out_path_id)

            if ulneighbor:
                # underlay neighbors don't need encapsulation routes
                if next_ip is None:
                    dest = self.node_by_id(dest)
                    yield UnderlayNeighborFwdEntry(dest)
            else:
                if next_ip != out_path_ip:
                    dest = self.node_by_id(dest)
                    out_path = [self.node_by_id(nid) for nid in out_path]
                    yield NodeIdEncapEntry(dest, out_path, out_path_id)
                if node.next_hop(out_path_ip) is None:
                    out_path = [self.node_by_id(nid) for nid in out_path]
                    yield PathIdFwdEntry(out_path, out_path_id)

    def topology_image(self, dpi: int = 200) -> io.BytesIO:
        nx.draw_networkx(self.topology, pos=self._pos, font_color="w")

        buffer = io.BytesIO()
        plt.savefig(buffer, format="png", transparent=True, dpi=dpi)
        return buffer

    def vicinity_image(self, node: KIRANode, dpi: int = 200) -> io.BytesIO:
        vicinity = [self.tid(n) for n in self.vicinity(node)]
        vicinity_edges = [(self.tid(u), self.tid(v)) for u, v in self.vicinity_edges(node)]
        root = self.tid(node)

        # draw other parts with their defaults first
        nx.draw_networkx(self.topology, pos=self._pos, font_color="w")

        # draw the vicinity
        nx.draw_networkx_nodes(
            self.topology, pos=self._pos, nodelist=vicinity, node_color="#8cb63c"
        )
        nx.draw_networkx_nodes(self.topology, pos=self._pos, nodelist=[root], node_color="#a22223")
        nx.draw_networkx_edges(
            self.topology, pos=self._pos, edgelist=vicinity_edges, edge_color="#8cb63c", width=2
        )

        buffer = io.BytesIO()
        plt.savefig(buffer, format="png", transparent=True, dpi=dpi)
        return buffer


class DebugShell[T](Cmd):
    intro = "Welcome to the debug shell of nesttest.  Type help or ? to list commands.\n"
    prompt = "ntest> "
    file = None

    def __init__(self, test: NestTest):
        super().__init__()
        self.test: NestTest[T] = test
        self.current_image: BaseImage | None = None

        self._compile_re()

    def _construct_replacement_map(self) -> Iterator[tuple[str, str]]:
        for node, node_cfg in self.test.nodes():
            nid = node_cfg.node_id
            ipv6 = node_cfg.ipv6
            short_nid = nid[:8]
            tid = node.name
            replacement = f"${tid}$"

            yield nid, f"{replacement:<{len(nid)}}"
            yield short_nid, f"{replacement:<{len(short_nid)}}"
            yield ipv6, f"{replacement:<{len(ipv6)}}"

    def _compile_re(self):
        self._replacement_map = dict(self._construct_replacement_map())
        replace_re = "|".join(re.escape(nid) for nid in self._replacement_map)
        ignore_case = f"(?i:{replace_re})"
        self._replace_re = re.compile(ignore_case)

    def sub_nid_name(self, string: str) -> str:
        """
        Substitute Node-IDs with the corresponding name used in the topology.

        Shortened Node-IDs of length 8
        and the IPv6-addresses of the nodes are also replaced.
        """

        def replace(match: re.Match):
            matched = match.group(0).lower()
            replace_with = self._replacement_map.get(matched, matched)
            return replace_with

        return self._replace_re.sub(replace, string)

    def _extract_node(self, arg: str) -> tuple[KIRANode | None, str | None]:
        """
        Get node in the next argument name.

        If the node can't be identified by its name the name will be placed
        in the second element of the return tuple.
        Otherwise the second element contains the remaining argument(s).
        """
        args = arg.split(maxsplit=1)
        if len(args) == 0:
            return (None, arg)

        node_name = args[0]
        scmd = args[1] if len(args) == 2 else None

        tid = self.test.name_tid_mapping.get(node_name)
        if tid is None:
            return None, arg
        node = self.test.node(tid)
        return (node, scmd)

    def do_pingall(self, arg):
        "Ping all nodes: PINGALL [-f,--failed] [-v,--verbose]"

        # process flags
        args = arg.split()
        failed = "-f" in args or "--failed" in args

        verbose = 2 if "-v" in args or "--verbose" in args else 0

        for x, _ in self.test.nodes():
            for y, y_config in self.test.nodes():
                if x != y:
                    print(f"Pinging {x:>3} --> {y:>3} ...", end="\r")
                    ip_y = y_config.ipv6
                    ip_y = Address(ip_y)
                    result = x.ping(ip_y, packets=1, verbose=verbose)

                    if not verbose:
                        if result:
                            # overwrite line if failed
                            end = "\r" if failed else "\n"
                            print(f"Pinging {x:>3} --> {y:>3} ✓  ", end=end, flush=True)
                            continue
                        else:
                            print(f"Pinging {x:>3} --> {y:>3} ✗   ", flush=True)

    def do_exec(self, arg):
        "Execute arbitrary command in the network namespace of node: EXEC <nid> <cmd>"
        node, cmd = self._extract_node(arg)
        if node is None:
            print(f"ERR: Node '{cmd}' not found.\nTo get a list of available nodes type NODES.")
            return
        if cmd is None:
            print("Provide a command to execute: EXECUTE <nid> <cmd>")
            return

        p = node.exec(cmd, logfile=sys.stdout)
        p.wait()
        print()

    def do_api(self, arg):
        "Issue arbitrary API call to node: API <nid> <rest_path>"
        node, path = self._extract_node(arg)
        if node is None:
            print(f"ERR: Node '{path}' not found.\nTo get a list of available nodes type NODES.")
            return
        if path is None:
            print("Provide an API-Path: API <nid> <rest_path>")
            return

        res = node.api_call(path)
        if res is None:
            print("ERR: API call failed")
            return
        res = self.sub_nid_name(res)
        print(res)

    def do_node_id(self, arg):
        "Obtain Node-Id: NODE_ID <nid>"
        node, _arg = self._extract_node(arg)
        if node is None:
            print(f"ERR: Node '{_arg}' not found.\nTo get a list of available nodes type NODES.")
            return

        tid = self.test.tid(node)
        assert tid is not None
        res = self.test.node_id(tid)
        print(res.hex())

    def do_store(self, arg):
        "Store a key-value pair in the DHT: STORE <nid> <key> <value>"
        node, key_data = self._extract_node(arg)
        if node is None:
            print(
                f"ERR: Node '{key_data}' not found.\nTo get a list of available nodes type NODES."
            )
            return
        if key_data is None:
            print("Provide a key and value: STORE <nid> <key> <value>")
            return

        key_data = key_data.split(maxsplit=1)
        if len(key_data) != 2:
            print("Provide a key and value: STORE <nid> <key> <value>")
            return
        key, data = key_data
        print(node.store(key, data))

    def do_fetch(self, arg):
        "Obtain value of a key in the DHT: FETCH <nid> <key>"
        node, key = self._extract_node(arg)
        if node is None:
            print(f"ERR: Node '{key}' not found.\nTo get a list of available nodes type NODES.")
            return
        if key is None:
            print("Provide a key to fetch: FETCH <nid> <key>")
            return

        res = node.fetch(key)
        if len(res) == 0:
            print(f"No value with key {key} found!")
        elif len(res) == 1:
            print(f"{key}={res[0]}")
        else:
            print(f"{key}=[")
            for value in res:
                print(f"    {value},")
            print("]")

    def do_routing_table(self, arg):
        "Dumps routing table of node: ROUTING_TABLE <nid>"
        node, _arg = self._extract_node(arg)
        if node is None:
            print(f"ERR: Node '{_arg}' not found.\nTo get a list of available nodes type NODES.")
            return

        res = node.routing_table()
        if res is None:
            print("ERR: API call failed")
            return
        res = self.sub_nid_name(res)
        print(res)

    def do_uln_table(self, arg):
        "Dump physical neighbor table of node: ULN_TABLE <nid>"
        node, _arg = self._extract_node(arg)
        if node is None:
            print(f"ERR: Node '{_arg}' not found.\nTo get a list of available nodes type NODES.")
            return

        res = node.uln_table()
        if res is None:
            print("ERR: API call failed")
            return
        res = self.sub_nid_name(res)
        print(res)

    def do_vicinity_graph(self, arg):
        "Dump vicinity graph of node: VICINITY_GRAPH <nid>"
        node, _arg = self._extract_node(arg)
        if node is None:
            print(f"ERR: Node '{_arg}' not found.\nTo get a list of available nodes type NODES.")
            return

        res = node.vicinity_graph()
        if res is None:
            print("ERR: API call failed")
            return
        res = self.sub_nid_name(res)
        print(res)

    def do_local_hashtable(self, arg):
        "Dump local hashtable of node: LOCAL_HASHTABLE <nid>"
        node, _arg = self._extract_node(arg)
        if node is None:
            print(f"ERR: Node '{_arg}' not found.\nTo get a list of available nodes type NODES.")
            return

        res = node.local_hashtable()
        if res is None:
            print("ERR: API call failed")
            return
        print(res)

    def do_checkup(self, arg):
        "Check if nodes are up: CHECKUP [nid]"
        # check all if no node is specified
        if arg == "":
            down_nodes = [n for n, _ in self.test.nodes() if not n.is_up()]
            if len(down_nodes) == 0:
                print("All nodes are up!")
            else:
                for n in down_nodes:
                    print(f"Node {n} is down.")

            return

        node, _arg = self._extract_node(arg)
        if node is None:
            print(f"ERR: Node '{_arg}' not found.\nTo get a list of available nodes type NODES.")
            return

        is_up = node.is_up()
        if is_up:
            print(f"Node {node} is up")
        else:
            print(f"Node {node} is not up")

    def do_next_ip(self, arg):
        node, ip = self._extract_node(arg)
        if node is None:
            print(f"ERR: Node '{ip}' not found.\nTo get a list of available nodes type NODES.")
            return

        next = node.next_ip(IPv6Address(ip))
        print()
        print(next)

    def do_next_hop(self, arg):
        node, ip = self._extract_node(arg)
        if node is None:
            print(f"ERR: Node '{ip}' not found.\nTo get a list of available nodes type NODES.")
            return

        next = node.next_hop(IPv6Address(ip))
        print()
        print(next)

    def do_path(self, arg):
        "Lookup Path-ID on the node: PATH <nid> [path-ip]"
        node, ip = self._extract_node(arg)
        if node is None:
            print(f"ERR: Node '{ip}' not found.\nTo get a list of available nodes type NODES.")
            return

        if ip is None:
            paths = node.paths_rt()
            if paths is None:
                return
        else:
            try:
                ip = IPv6Address(ip)
            except AddressValueError:
                print(f"ERR: Path-IP {ip} is not a valid IPv6-address")
                return
            assert ip in PATH_IP

            path = node.path(ip)
            if path is None:
                print(f"ERR: Unknown PathIP: {ip}")
                return
            paths = [path]

        for path in paths:
            path = list(path)
            path_id = KIRANode.path_id(path)
            path = ", ".join(
                (
                    str(self.test.node_by_ip(IPv6Address(bytes.fromhex("fc00") + hop)))
                    for hop in path
                )
            )
            path_ip = IPv6Address(bytes.fromhex("fcaa") + path_id)
            print(f"{path_ip} ==> {path}")

    def do_traceroute(self, arg):
        "Traceroute (all) forwarding path: TRACEROUTE [<nid_x> <nid_y>]"

        # trace __all__ paths
        if arg == "":
            for x_tid in self.test.topology:
                for y_tid in self.test.topology:
                    success = self.test.traceroute(x_tid, y_tid, verbose=False)
                    # only show unsuccessful traceroutes
                    if not success:
                        self.test.traceroute(x_tid, y_tid, verbose=True)
                        print()
                        print("==============================================")
                        print()

            return

        x, arg = self._extract_node(arg)
        if x is None:
            print(f"ERR: Node '{x}' not found.\nTo get a list of available nodes type NODES.")
            return
        if arg is None:
            print("Destination not found. Usage: TRACEROUTE <nid_x> <nid_y>")
            return

        y, _arg = self._extract_node(arg)
        if y is None:
            print(f"ERR: Node '{_arg}' not found.\nTo get a list of available nodes type NODES.")
            return
        x_tid = self.test.tid(x)
        assert x_tid is not None
        y_tid = self.test.tid(y)
        assert y_tid is not None

        self.test.traceroute(x_tid, y_tid, verbose=True)

    def do_link(self, arg):
        "Sets link (or all links of node) up or down: LINK <DOWN/UP> <nid_x> [nid_y]"

        # parse args
        mode, arg = arg.split(maxsplit=1)
        x, arg = self._extract_node(arg)
        if x is None:
            print(f"ERR: Node '{x}' not found.\nTo get a list of available nodes type NODES.")
            return

        x_tid = self.test.tid(x)
        assert x_tid is not None
        if arg is not None:
            y, _arg = self._extract_node(arg)
            if y is None:
                print(
                    f"ERR: Node '{_arg}' not found.\nTo get a list of available nodes type NODES."
                )
                return
            y_tid = self.test.tid(y)
            assert y_tid is not None
            ys_tid = [y_tid]
        else:
            # all links from x
            ys_tid = [y_tid for _x, y_tid in self.test.topology.edges(x_tid)]

        mode = mode.lower()
        if mode == "up":
            for y_tid in ys_tid:
                y = self.test.topology.nodes[y_tid]["node"]
                print(f"{x:>3} -✔- {y:>3} ...")
                self.test.link(x_tid, y_tid).up()
        elif mode == "down":
            for y_tid in ys_tid:
                y = self.test.topology.nodes[y_tid]["node"]
                print(f"{x:>3} -✗- {y:>3} ...")
                self.test.link(x_tid, y_tid).down()
        else:
            print(f"ERR: Unknown mode {mode}")

    def do_down(self, arg):
        "Alias for LINK DOWN <...>"
        self.do_link(f"DOWN {arg}")

    def do_up(self, arg):
        "Alias for LINK UP <...>"
        self.do_link(f"UP {arg}")

    def do_links(self, arg):
        "List links in topology: LINKS [nid]"

        if arg == "":
            tid = None
        else:
            node, _arg = self._extract_node(arg)
            if node is None:
                print(
                    f"ERR: Node '{_arg}' not found.\nTo get a list of available nodes type NODES."
                )
                return
            tid = self.test.tid(node)

        for x, y, link in self.test.links(tid):
            x = self.test.node(x)
            y = self.test.node(y)
            up_indicator = "-" if link.is_up() else "✗"
            print(f"{x:>3} -{up_indicator}- {y:>3}")
        return

    def do_edges(self, arg):
        "Alias for LINKS: EDGES [nid]"
        self.do_links(arg)

    def do_nodes(self, arg):
        "List all nodes present in the topology: NODES"
        for n, _ in self.test.nodes():
            nid = n.node_id()
            nid = nid.hex().upper() if nid else "???"
            print(f"{n:>3} {nid}")

    def do_vicinity(self, arg):
        "Get vicinity of <nid>: VICINITY <nid>"
        node, _arg = self._extract_node(arg)
        if node is None:
            print(f"ERR: Node '{_arg}' not found.\nTo get a list of available nodes type NODES.")
            return

        # sort topo-ids like `k17` in expected order
        def _srt(n):
            match = re.match(r"([a-zA-Z]+)(\d+)", n.name)
            if match:
                alpha, num = match.groups()
                return (alpha, int(num))
            return (n, 0)

        def _sorted(iter: Iterable[KIRANode]) -> Iterable[KIRANode]:
            return sorted(iter, key=_srt)

        # nodes
        buffer = self.test.vicinity_image(node, dpi=100)
        img = Image.open(buffer)
        self.current_image = AutoImage(img)
        self.current_image.set_size(height=20)
        self.current_image.draw(h_align="left", v_align="top", pad_height=1)

        vicinity = self.test.vicinity(node)
        print("Vicinity:")
        for v in _sorted(vicinity):
            print(f"{v:>3}")
        print()

        unknowns = self.test.unknown_vicinity(node)
        if unknowns is not None:
            unknowns = set(unknowns)
            if len(unknowns) != 0:
                dvicinity = self.test.discovered_vicinity(node)
                print("Discovered Vicinity:")
                if dvicinity is None:
                    print("ERR: determining known vicinity.")
                else:
                    for v in _sorted(set(dvicinity)):
                        print(f"{v:>3}")
                    print()

                print("Missing Nodes in the Vicinity Graph:")
                for unknown in _sorted(unknowns):
                    print(f"{unknown:>3}")
            else:
                print("All vicinity nodes where discoverd.")
        else:
            print("ERR: checking on (un)known vicinity of the node.")
        print()

        unknown_edges = self.test.unknown_vicinity_edges(node)
        if unknown_edges is not None:
            unknown_edges = set(unknown_edges)
            if len(unknown_edges) != 0:
                print("Unknown Vicinity Edges:")
                for u, v in unknown_edges:
                    print(f"{u:>3} -?- {v:>3}")
            else:
                print("All edges in the vicinity where discovered.")
        else:
            print("ERR: checking on (un)known vicinity of the node.")
        print()

        def _print_fwd_entry(entry: FwdEntry) -> None:
            match entry:
                case UnderlayNeighborFwdEntry(n):
                    print(f"{n} -> fe80::/64")
                case NodeIdEncapEntry(n, path, pid):
                    pid = pid.hex()
                    path = ", ".join(str(n) for n in path)

                    print(f"{n} -> {pid} ({path})")
                case PathIdFwdEntry(path, pid):
                    pid = pid.hex()
                    path = ", ".join(str(n) for n in path)

                    print(f"{pid} -> fc00::/112                   [ {path} ]")
                case PathIdEncapEntry(in_path, ipid, out_path, opid):
                    ipid = ipid.hex()
                    in_path = ", ".join(str(n) for n in in_path)
                    opid = opid.hex()
                    out_path = ", ".join(str(n) for n in out_path)

                    print(f"{ipid} -> {opid} [ {in_path} -> {out_path} ] ")

        missing_entries = self.test.missing_pathsetups(node)
        if missing_entries is not None:
            missing_entries = list(missing_entries)
            if len(missing_entries) != 0:
                print("Missing path setups:")
                for entry in missing_entries:
                    print("  - ", end="")
                    _print_fwd_entry(entry)
            else:
                print("All paths inside the vicinity are setup.")
        else:
            print("ERR: checking on missing path setups.")
        print()

        missing_fwd_entries = self.test.missing_pathsetups_rt(node)
        if missing_fwd_entries is not None:
            missing_fwd_entries = list(missing_fwd_entries)
            if len(missing_fwd_entries) != 0:
                print("Missing paths or nodes setups of Contacts in the Routing Table:")
                for entry in missing_fwd_entries:
                    print("  - ", end="")
                    _print_fwd_entry(entry)

            else:
                print("All paths and nodes of Contacts in the Routing Table are setup.")
        else:
            print("ERR: checking on missing path setups.")
        print()

        additional_vicinity = self.test.additional_vicinity(node)
        if additional_vicinity is not None:
            additional_vicinity = set(additional_vicinity)
            if len(additional_vicinity) != 0:
                print("Additional Vicinity:")
                for additional in _sorted(additional_vicinity):
                    print(f"{additional:>3}")
            else:
                print("No unexpected nodes in the vicinity.")
        else:
            print("ERR: checking on additional vicinity of the node.")
        print()

    def do_checkvicinity(self, arg):
        "Check if whole vicinity is known by <nid>: CHECKVICINITY [nid]"
        if arg == "":
            nodes = (node for node, _ in self.test.nodes())
        else:
            node, _arg = self._extract_node(arg)
            if node is None:
                print(
                    f"ERR: Node '{_arg}' not found.\nTo get a list of available nodes type NODES."
                )
                return
            nodes = iter([node])

        fail = False

        buffer = self.test.topology_image(dpi=100)
        img = Image.open(buffer)
        self.current_image = AutoImage(img)
        self.current_image.set_size(height=20)
        self.current_image.draw(h_align="left", v_align="top", pad_height=1)

        print("Node: v e p r a")
        for node in nodes:
            # TODO: Check for evidence in nftables that all paths are setup
            # TODO: Check if all discovered neighbors of a vicinity node match

            unknown_vicinity = self.test.unknown_vicinity(node)
            if unknown_vicinity is None:
                print(f"{node:<3} ERR determining known vicinity.")
                return
            try:
                next(unknown_vicinity)
                unknown_vicinity = True
                fail = True
            except StopIteration:
                unknown_vicinity = False
            unknown_edges = self.test.unknown_vicinity_edges(node)
            if unknown_edges is None:
                print(f"{node:<3} ERR determining known vicinity edges.")
                return
            try:
                next(unknown_edges)
                unknown_edges = True
                fail = True
            except StopIteration:
                unknown_edges = False

            missing_pathsetups = self.test.missing_pathsetups(node)
            if missing_pathsetups is None:
                print(f"{node:<3} ERR determining paths setup.")
                return
            try:
                next(missing_pathsetups)
                missing_pathsetups = True
                fail = True
            except StopIteration:
                missing_pathsetups = False

            missing_pathsetups_rt = self.test.missing_pathsetups_rt(node)
            if missing_pathsetups_rt is None:
                print(f"{node:<3} ERR determining paths setup.")
                return
            try:
                next(missing_pathsetups_rt)
                missing_pathsetups_rt = True
                fail = True
            except StopIteration:
                missing_pathsetups_rt = False

            additional_vicinity = self.test.additional_vicinity(node)
            if additional_vicinity is None:
                print(f"{node:<3} ERR determining known vicinity.")
                return
            try:
                next(additional_vicinity)
                additional_vicinity = True
                fail = True
            except StopIteration:
                additional_vicinity = False

            def _print_bool(b: bool) -> None:
                if b:
                    print("✗ ", end="")
                else:
                    print("✔ ", end="")

            print(f"{node:<3} : ", end="")
            _print_bool(unknown_vicinity)
            _print_bool(unknown_edges)
            _print_bool(missing_pathsetups)
            _print_bool(missing_pathsetups_rt)
            _print_bool(additional_vicinity)
            print()

        print()
        print("v = Vicinity-Nodes with SSN in Vicinty Graph")
        print("e = Edges inside the vicinity radius")
        print("p = Paths inside the vicinity radius setup (nftables)")
        print("r = Paths of Contacts inside the Routing Table setup")
        print("a = Additional Nodes in Vicinity Graph")

        if fail:
            print()
            print("To further investigate failures type: VICINITY <nid>")

    def do_netns(self, arg):
        "Get network namespace name of node <nid>: NETNS [nid]"
        node, _arg = self._extract_node(arg)
        if node is None:
            print(f"ERR: Node '{_arg}' not found.\nTo get a list of available nodes type NODES.")
            return

        netns_name = node.id
        print(f"netnsname: {netns_name}")

    def do_exit(self, arg):
        "Exit the debug shell"
        print("Exiting...")
        # Kill all processes in the network namespaces to make sure everything is cleaned up
        for node, _ in self.test.nodes():
            netns_name = node.id
            p = Popen(f"ip netns pids {netns_name}", shell=True, stdout=PIPE)
            stdout, _ = p.communicate()
            if p.returncode == 0 and stdout:
                pids = stdout.decode().strip().split()
                if pids:
                    kill_cmd = f"kill {' '.join(pids)}"
                    killer = Popen(
                        f"ip netns exec {netns_name} {kill_cmd}", shell=True
                    ).communicate()

        # TODO: still partially broken, python processes are not killed for some reason

        self.close()
        return True

    def cmdloop(self, intro: Any | None = None) -> None:
        try:
            super().cmdloop(intro)
        except KeyboardInterrupt:
            self.do_exit(None)

    # ----- record and playback -----

    def do_record(self, arg):
        "Save future commands to filename:  RECORD rose.cmd"
        self.file = open(arg, "w")

    def do_playback(self, arg):
        "Playback commands from a file:  PLAYBACK rose.cmd"
        self.close()
        with open(arg) as f:
            self.cmdqueue.extend(f.read().splitlines())

    def do_show_graph(self, arg):
        "Show the current graph of the topology: SHOW_GRAPH"

        buffer = self.test.topology_image(dpi=400)
        img = Image.open(buffer)
        self.current_image = AutoImage(img)
        self.current_image.draw()

    def precmd(self, line):
        line = line.lower()
        if self.file and "playback" not in line:
            print(line, file=self.file)
        self.current_image = None
        return line

    def close(self):
        if self.file:
            self.file.close()
            self.file = None


def main(args):
    # Load the configuration from the GML file
    G = nx.readwrite.read_gml(args.test_gml)

    for node in G.nodes:
        cfg = NodeConfig(**G.nodes[node]["config"])
        # enable otel for all nodes
        if args.otel:
            cfg.otel = True

        G.nodes[node]["config"] = cfg

    # Create and run the test
    test = NestTest(G)
    DebugShell(test).cmdloop()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Nest Test Script")
    parser.add_argument("test_gml", type=str, help="The gml file")
    parser.add_argument(
        "--otel",
        action="store_true",
        help="Enable open telemetry exports on all nodes",
    )
    args = parser.parse_args()
    main(args)
