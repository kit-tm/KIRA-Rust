import argparse
import os
from cmd import Cmd
import sys
import re
from subprocess import Popen, PIPE
from typing import Iterable, Iterator
import json
import base64
from hashlib import sha1
from ipaddress import IPv6Address, IPv6Network, AddressValueError

import networkx as nx
from networkx import Graph

import nest
from nest.topology import Node, Address, Interface, connect


from common import NodeConfig


PATH_IP: IPv6Network = IPv6Network("fcaa::/16")
NODE_IP: IPv6Network = IPv6Network("fc00::/16")

VICINITY_RADIUS: int = 3


class KIRANode(Node):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)

        self._api_port = 8080

    def exec(self, cmd: str, env_vars=None, logfile=None) -> Popen:
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

    def pn_table(self) -> str | None:
        path = "_dev/pn-table"
        return self.api_call(path)

    def vicinity_graph(self) -> str | None:
        path = "_dev/vicinity-graph"
        return self.api_call(path)

    def local_hashtable(self) -> str | None:
        path = "dht/_dev/local-hashtable"
        return self.api_call(path)

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
                path_ip = IPv6Address(result["encap"]["dst"])
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

    def paths(self) -> Iterator[Iterator[bytes]]:
        """Returns an iterator over all paths (node-id sequence) known by the node.

        Usually all paths known by the node are paths in the RoutingTable.
        """
        routing_table = self.routing_table()
        # join lines

        # find all paths
        r_path = re.compile(r"Path\s+{\s+ids:\s+\[([\s\w(:),]+)\]", flags=re.MULTILINE)
        r_nid = re.compile(r"NodeId\(([\w]+)\),")

        for path_match in r_path.finditer(routing_table):
            path_str = path_match.group(1)
            hops = (bytes.fromhex(nid_match.group(1)) for nid_match in r_nid.finditer(path_str))
            yield hops

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

        paths = self.paths()
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


class NestTest[T]:  # T = tid type, usually int or str
    """
    Nest Test
    ===========

    This is a test for the Nest framework. It creates a topology and runs a connectivity test between nodes.

    Parameters
    ----------
    config : dict
        The configuration for the test.
    """

    def __init__(self, config: Graph):
        self.topology: Graph = config
        self.name_tid_mapping: dict[str, T] = dict()

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

            logfile = f"{node}.log"
            env_vars = os.environ.copy()
            env_vars["RUST_LOG_STYLE"] = "never"
            env_vars["NO_COLOR"] = "1"
            env_vars["RUST_LOG"] = env_vars.get("RUST_LOG", "info")
            env_vars["RUST_BACKTRACE"] = "1"
            with open(logfile, "w") as f:
                node.exec(
                    f"./target/debug/kirad --root-id {node_id} --nftables-conf ./kirad/conf/nftables.conf",
                    logfile=f,
                    env_vars=env_vars,
                )

    def tid(self, node: KIRANode) -> T | None:
        name = node.name
        return self.name_tid_mapping.get(name)

    def node(self, tid) -> KIRANode:
        return self.topology.nodes[tid]["node"]

    def node_by_ip(self, ip: IPv6Address) -> KIRANode | None:
        assert ip in NODE_IP
        for _, data in self.topology.nodes(data=True):
            config = data["config"]
            n_ip = IPv6Address(config.ipv6)
            if n_ip == ip:
                return data["node"]

    def nodes(self) -> Iterator[tuple[KIRANode, NodeConfig]]:
        return ((ndata["node"], ndata["config"]) for _, ndata in self.topology.nodes(data=True))

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
            return "??? Unkown Action ???"

    def traceroute(self, x_tid: T, y_tid: T, maxhops: int = 10, verbose: bool = False) -> bool:
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

    def vicinity(self, node: KIRANode) -> set[KIRANode]:
        tid = self.name_tid_mapping[node.name]
        paths = nx.single_source_shortest_path(self.topology, tid, cutoff=VICINITY_RADIUS)
        reachable_nodes = {self.node(reachable) for reachable in paths.keys()}
        return reachable_nodes

    def unknown_vicinity(self, node: KIRANode) -> Iterator[KIRANode]:
        vicinity = self.vicinity(node)

        known_vicinity_raw = node.vicinity_graph()
        if known_vicinity_raw is None:
            return set()
        node_id_re = re.compile(r"NodeId\((\w+)\)")
        matches = node_id_re.finditer(known_vicinity_raw)

        for match in matches:
            nid = bytes.fromhex(match.group(1))
            nip = IPv6Address(bytes.fromhex("fc00") + nid)
            n = self.node_by_ip(nip)

            if n is not None and n not in vicinity:
                yield node


class DebugShell[T](Cmd):
    intro = "Welcome to the debug shell of nesttest.  Type help or ? to list commands.\n"
    prompt = "(debug)"
    file = None

    test: NestTest[T]

    def __init__(self, test: NestTest):
        super().__init__()
        self.test = test

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
        replace_re = "|".join(re.escape(nid) for nid in self._replacement_map.keys())
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
        if "-f" in args or "--failed" in args:
            failed = True
        else:
            failed = False

        if "-v" in args or "--verbose" in args:
            verbose = 2
        else:
            verbose = 0

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
            print((f"ERR: Node '{cmd}' not found.\nTo get a list of available nodes type NODES."))
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
            print((f"ERR: Node '{path}' not found.\nTo get a list of available nodes type NODES."))
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

    def do_store(self, arg):
        "Store a key-value pair in the DHT: STORE <nid> <key> <value>"
        node, key_data = self._extract_node(arg)
        if node is None:
            print(
                (f"ERR: Node '{key_data}' not found.\nTo get a list of available nodes type NODES.")
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
            print((f"ERR: Node '{key}' not found.\nTo get a list of available nodes type NODES."))
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
            print((f"ERR: Node '{_arg}' not found.\nTo get a list of available nodes type NODES."))
            return

        res = node.routing_table()
        if res is None:
            print("ERR: API call failed")
            return
        res = self.sub_nid_name(res)
        print(res)

    def do_pn_table(self, arg):
        "Dump physical neighbor table of node: PN_TABLE <nid>"
        node, _arg = self._extract_node(arg)
        if node is None:
            print((f"ERR: Node '{_arg}' not found.\nTo get a list of available nodes type NODES."))
            return

        res = node.pn_table()
        if res is None:
            print("ERR: API call failed")
            return
        res = self.sub_nid_name(res)
        print(res)

    def do_vicinity_graph(self, arg):
        "Dump vicinity graph of node: VICINITY_GRAPH <nid>"
        node, _arg = self._extract_node(arg)
        if node is None:
            print((f"ERR: Node '{_arg}' not found.\nTo get a list of available nodes type NODES."))
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
            print((f"ERR: Node '{_arg}' not found.\nTo get a list of available nodes type NODES."))
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
            print((f"ERR: Node '{_arg}' not found.\nTo get a list of available nodes type NODES."))
            return

        is_up = node.is_up()
        if is_up:
            print(f"Node {node} is up")
        else:
            print(f"Node {node} is not up")

    def do_next_ip(self, arg):
        node, ip = self._extract_node(arg)
        if node is None:
            print((f"ERR: Node '{ip}' not found.\nTo get a list of available nodes type NODES."))
            return

        next = node.next_ip(IPv6Address(ip))
        print()
        print(next)

    def do_next_hop(self, arg):
        node, ip = self._extract_node(arg)
        if node is None:
            print((f"ERR: Node '{ip}' not found.\nTo get a list of available nodes type NODES."))
            return

        next = node.next_hop(IPv6Address(ip))
        print()
        print(next)

    def do_path(self, arg):
        "Lookup Path-ID on the node: PATH <nid> [path-ip]"
        node, ip = self._extract_node(arg)
        if node is None:
            print((f"ERR: Node '{ip}' not found.\nTo get a list of available nodes type NODES."))
            return

        if ip is None:
            paths = node.paths()
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
            print((f"ERR: Node '{x}' not found.\nTo get a list of available nodes type NODES."))
            return
        if arg is None:
            print("Destination not found. Usage: TRACEROUTE <nid_x> <nid_y>")
            return

        y, _arg = self._extract_node(arg)
        if y is None:
            print((f"ERR: Node '{_arg}' not found.\nTo get a list of available nodes type NODES."))
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
            print((f"ERR: Node '{x}' not found.\nTo get a list of available nodes type NODES."))
            return

        x_tid = self.test.tid(x)
        assert x_tid is not None
        if arg is not None:
            y, _arg = self._extract_node(arg)
            if y is None:
                print(
                    (f"ERR: Node '{_arg}' not found.\nTo get a list of available nodes type NODES.")
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
                    (f"ERR: Node '{_arg}' not found.\nTo get a list of available nodes type NODES.")
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
            print(f"{n:>3}")

    def do_vicinity(self, arg):
        "Get vicinity of <nid>: VICINITY <nid>"
        node, _arg = self._extract_node(arg)
        if node is None:
            print((f"ERR: Node '{_arg}' not found.\nTo get a list of available nodes type NODES."))
            return

        vicinity = self.test.vicinity(node)
        print()
        print(vicinity)

        unknown_vicinity = list(self.test.unknown_vicinity(node))
        print()
        print(unknown_vicinity)

    def do_checkvicinity(self, arg):
        "Check if whole vicinity is known by <nid>: CHECKVICINITY [nid]"
        if arg == "":
            nodes = (node for node, _ in self.test.nodes())
        else:
            node, _arg = self._extract_node(arg)
            if node is None:
                print(
                    (f"ERR: Node '{_arg}' not found.\nTo get a list of available nodes type NODES.")
                )
                return
            nodes = iter([node])

        for node in nodes:
            unknown_vicinity = list(self.test.unknown_vicinity(node))

            if len(unknown_vicinity) == 0:
                print(f"{node:<3} discovered its vicinity completely")
            else:
                print(f"{node:<3} does not know the following nodes:")
                for unknown in unknown_vicinity:
                    print(f"{unknown:<3}")

    def do_netns(self, arg):
        "Get network namespace name of node <nid>: NETNS [nid]"
        node, _arg = self._extract_node(arg)
        if node is None:
            print((f"ERR: Node '{_arg}' not found.\nTo get a list of available nodes type NODES."))
            return

        netns_name = node.id
        print(f"netnsname: {netns_name}")

    def do_exit(self, arg):
        "Exit the debug shell"
        print("Exiting...")
        self.close()
        return True

    def cmdloop(self):
        try:
            super().cmdloop()
        except KeyboardInterrupt:
            return self.do_exit(None)

    # ----- record and playback -----

    def do_record(self, arg):
        "Save future commands to filename:  RECORD rose.cmd"
        self.file = open(arg, "w")

    def do_playback(self, arg):
        "Playback commands from a file:  PLAYBACK rose.cmd"
        self.close()
        with open(arg) as f:
            self.cmdqueue.extend(f.read().splitlines())

    def precmd(self, line):
        line = line.lower()
        if self.file and "playback" not in line:
            print(line, file=self.file)
        return line

    def close(self):
        if self.file:
            self.file.close()
            self.file = None


def main(args):
    # Load the configuration from the GML file
    G = nx.readwrite.read_gml(args.test_gml)

    for node in G.nodes:
        G.nodes[node]["config"] = NodeConfig(**G.nodes[node]["config"])

    # Create and run the test
    test = NestTest(G)
    DebugShell(test).cmdloop()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Nest Test Script")
    parser.add_argument("test_gml", type=str, help="The gml file")
    args = parser.parse_args()
    main(args)
