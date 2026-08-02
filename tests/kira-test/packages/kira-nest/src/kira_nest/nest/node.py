from __future__ import annotations

import base64
import importlib.resources
import json
import logging
import os
import pathlib
import re
import sys
from collections.abc import Iterator
from dataclasses import dataclass
from functools import cached_property, singledispatchmethod
from ipaddress import (
    AddressValueError,
    IPv6Address,
    IPv6Network,
)
from itertools import chain
from subprocess import PIPE, Popen
from typing import Any
from urllib.parse import urlencode

import networkx as nx
from kira_common.domain import (
    NODE_IP_SN,
    PATH_IP_SN,
    VICINITY_RADIUS,
    KiraIP,
    NodeID,
    NodeIP,
    Path,
    PathID,
    PathIP,
)
from kira_common.domain.forwarding import (
    FwdEntry,
    NodeIDEncapEntry,
    NodeIDFwdEntry,
    PathIDEntry,
    PathIDFwdEntry,
    PathIDSwapEntry,
)
from kira_common.node_config import NodeConfig
from kira_common.paths import NFTABLES_CONF
from nest.topology import Address, Node
from networkx import Graph

logger = logging.getLogger(__name__)


@dataclass(frozen=True)
class KIRANodeApi:
    """
    Raw access to the KIRA debug API of the node.

    Parsed outputs are exposed by the KIRANode class.
    """

    node: KIRANode
    _api_port: int = 8080

    def call(
        self,
        path: str,
        params: dict[str, str] | None = None,
        payload: str | None = None,
    ) -> str | None:
        cmd = f"curl localhost:{self._api_port}/{path}"
        if params is not None:
            # sadly curl doesn't support encoding params inside an url
            # using --data-urlencode key=value on a POST method-call
            cmd += "?" + urlencode(params)
        if payload:
            cmd += f" --data-raw '{payload}'"

        p = self.node.exec(cmd, logfile=PIPE)
        stdout, _ = p.communicate()
        return stdout.decode("utf-8") if p.returncode == 0 else None

    def store(self, key: str, data: str) -> str | None:
        path = "dht/store"
        return self.call(path, {"key": key}, data)

    def fetch(self, key: str) -> list[str]:
        path = "dht/fetch"
        res = self.call(path, {"key": key})
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
        return self.call(path)

    def uln_table(self) -> str | None:
        path = "_dev/uln-table"
        return self.call(path)

    def vicinity_graph(self) -> str | None:
        path = "_dev/vicinity-graph"
        return self.call(path)

    def local_hashtable(self) -> str | None:
        path = "dht/_dev/local-hashtable"
        return self.call(path)

    def node_id(self) -> NodeID | None:
        path = "node-id"
        res = self.call(path)
        if res is None:
            return None
        res = json.loads(res)
        return NodeID.fromhex(res.get("node-id"))


class KIRANode(Node):
    ENV_LOG_PATH = "KIRAD_LOG_PATH"

    def __init__(
        self,
        config: NodeConfig,
    ) -> None:
        super().__init__(config.name)

        self.config: NodeConfig = config
        self.__node_id: NodeID | None = None

    def exec(
        self, cmd: str, env_vars: dict | None = None, logfile: Any = None
    ) -> Popen:
        """
        Execute a command in the node's namespace.
        """
        if env_vars is None:
            env_vars = os.environ.copy()
        if logfile is None:
            logger.debug("No logfile provided, using stdout")
        return Popen(
            f"ip netns exec {self.id} {cmd}",
            shell=True,
            env=env_vars,
            stdout=logfile,
            stderr=logfile,
        )

    @cached_property
    def api(self) -> KIRANodeApi:
        return KIRANodeApi(self)

    @property
    def log_path(self) -> pathlib.Path:
        raw_log_path = os.environ.get(self.ENV_LOG_PATH)
        if raw_log_path is None:
            log_path = pathlib.Path("log").resolve()
        else:
            log_path = pathlib.Path(raw_log_path).resolve()
        log_path.mkdir(parents=True, exist_ok=True)
        return log_path

    def start(
        self, binary: pathlib.Path, wrapper: str | None = None, *args: str
    ) -> Popen:
        logfile = self.log_path / f"{self:02}.log"
        env_vars = os.environ.copy()
        env_vars["RUST_LOG_STYLE"] = "never"
        env_vars["NO_COLOR"] = "1"
        env_vars["RUST_LOG"] = env_vars.get("RUST_LOG", "info")
        env_vars["RUST_BACKTRACE"] = "1"

        arg: str = " ".join(args)
        with (
            open(logfile, "w") as f,
            importlib.resources.as_file(NFTABLES_CONF) as nftables_conf,
        ):
            wrapper = f"{wrapper} -- " if wrapper else ""
            return self.exec(
                f"{wrapper}'{binary}' --root-id '{self.config.node_id}'"
                f" --nftables-conf {nftables_conf} {arg} && exit",
                logfile=f,
                env_vars=env_vars,
            )

    def is_up(self) -> bool:
        path = "node-id"
        res = self.api.call(path)
        if res is None:
            return False
        res = json.loads(res)
        return "node-id" in res

    def is_down(self) -> bool:
        return not self.is_up()

    def ping(
        self,
        destination_address: Address,
        preload: int = 1,
        packets: int = 5,
        verbose: int = 2,
        timeout: int = 1,
    ) -> bool:
        # overwrite ping to support timeout
        dst_addr = destination_address.get_addr(with_subnet=False)
        if verbose not in [0, 1, 2]:
            raise ValueError(
                f"Verbose parameter value is {verbose}. It should be 0, 1 or 2."
            )

        if verbose == 2:  # noqa: PLR2004
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

    @property
    def node_id(self) -> NodeID:
        # cache NodeID of running KIRA instance
        if self.__node_id is not None:
            return self.__node_id

        # WARNING: Restarting KIRA daemon with different ID
        # will result in outdated information
        self.__node_id = self.api.node_id()
        return self.__node_id or NodeID.fromhex(self.config.node_id)

    @singledispatchmethod
    def next_ip(self, ip: KiraIP) -> KiraIP | None:
        """
        Returns next KiraIP used for forwarding of the packet.
        Usually this IP changes per hop but can remain the same,
        if the packet is not encapsulated but forwarded unchanged to the next hop.
        """
        _ = ip
        raise NotImplementedError("next_ip only implemented for NodeIP or PathIP")

    @next_ip.register
    def next_ip_encap(self, ip: PathIP) -> KiraIP | None:
        """Determines the next KiraIP of an encapsulated packet (with PathIP)."""

        # lookup nftables forwardmap for translation
        cmd = f"nft get element ip6 kira forwardmap {{ {ip} }}"
        p = self.exec(cmd, logfile=PIPE)
        stdout, _ = p.communicate()
        if p.returncode != 0:
            return None
        stdout = stdout.decode("utf-8")

        # parse output
        match = re.search(r"elements = { [a-fA-F0-9:]+ : ([a-fA-F0-9:]+) }", stdout)
        if match:
            next_ip = IPv6Address(match.group(1))
            # label swap or decap
            if next_ip in PATH_IP_SN:
                return PathIP(next_ip)
            if next_ip in NODE_IP_SN:
                return NodeIP(next_ip)

        logger.error(f"{ip} unknown to node {self}: Parsing of output failed: {stdout}")
        return None

    @next_ip.register
    def next_ip_decap(self, ip: NodeIP) -> KiraIP | None:
        """
        Determines the next KiraIP of an decapsulated packet (without PathIP).
        """

        # lookup ip route
        cmd = f"ip -6 -j route get {ip}"
        p = self.exec(cmd, logfile=PIPE)
        stdout, _ = p.communicate()
        if p.returncode != 0:
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
            assert path_ip in PATH_IP_SN
            return PathIP(path_ip)

        # OPTION 2: it's us
        if result.get("type") == "local":
            # IP technically doesn't change
            return ip

        # OPTION 3: physical neighbor
        if "dst" in result and ip in IPv6Network(result["dst"]):
            return ip

        logger.error(f"Unexpected route for {ip} on node {self}: {result}")
        return None

    # TODO: reduce complexity
    def next_hop(self, ip: KiraIP) -> NodeIP | None:  # noqa: PLR0911,PLR0912 -- ignore complexity warnings
        """
        Returns NodeIP of the next hop the packet will be forwarded to.

        The supplied IP is the egress IP of the packet after it has
        been processed by the node (relabeling, encapsulation, ...).
        To determine the egress IP use `self.next_ip`.
        """

        # lookup ip route
        cmd = f"ip -6 -j route get {ip}"
        p = self.exec(cmd, logfile=PIPE)
        stdout, _ = p.communicate()
        if p.returncode != 0:
            return None
        stdout = stdout.decode("utf-8")

        # parse output
        result = json.loads(stdout)
        assert len(result) == 1
        result = result[0]

        # OPTION 1: it's us
        if result.get("type") == "local":
            assert IPv6Address(result["prefsrc"]) == ip
            assert ip in NODE_IP_SN
            return NodeIP(ip)

        # OPTION 2: Forward VIA next hop
        elif "gateway" in result:  # gateway == via :/
            gateway = IPv6Address(result["gateway"])
            if gateway in NODE_IP_SN:
                return NodeIP(gateway)

            # in newer implementation we use LL-IPv6 as gateway
            assert gateway.is_link_local
            dev = result.get("dev")

            cmd = "ip -6 -j route show"
            p = self.exec(cmd, logfile=PIPE)
            stdout, _ = p.communicate()
            if p.returncode != 0:
                logger.error(f"{ip} unknown to node {self}")
                return None
            # parse output
            stdout = stdout.decode("utf-8")
            routes = json.loads(stdout)

            # find NODE_IP dev <dev> entry to obtain NODE_IP
            next_hop = None
            for route in routes:
                if route.get("dev") == dev and route.get("protocol") == "static":
                    dst = route.get("dst")
                    try:
                        dst = IPv6Address(dst)
                    except AddressValueError:
                        continue

                    if dst in NODE_IP_SN:
                        # HACK: this only works in unswitched networks
                        #       where one node is reachable per interface
                        if next_hop is not None:
                            raise NotImplementedError(
                                "Finding next hop in switched networks not implemented"
                            )
                        next_hop = NodeIP(dst)
            return next_hop

        # OPTION 3: Overlay Hop -- (re)encapsulation on this node required
        elif "encap" in result:
            assert ip in NODE_IP_SN

            # determine own ip
            if "prefsrc" in result:
                node_ip = IPv6Address(result["prefsrc"])
                assert node_ip in NODE_IP_SN
                return NodeIP(node_ip)

            return self.node_id.to_node_ip()

        # OPTION 4: Physical neighbor
        elif "dst" in result and IPv6Address(result["dst"]) == ip:
            assert ip in NODE_IP_SN
            return NodeIP(ip)

        logger.error(f"Unexpected route for {ip} on node {self}: {result}")
        return None

    def paths_routing_table(self) -> Iterator[Path] | None:
        """Returns an iterator over all active routing table paths."""
        routing_table = self.api.routing_table()
        if routing_table is None:
            return None

        # find all paths
        r_path = re.compile(
            r"active_path:\s+Some\(\s+Path\s+{\s+ids:\s+\[([\s\w(:),]+)\]",
            flags=re.MULTILINE,
        )
        r_nid = re.compile(r"NodeId\(([\w]+)\),")

        for path_match in r_path.finditer(routing_table):
            path_str = path_match.group(1)
            yield Path(
                NodeID.fromhex(nid_match.group(1))
                for nid_match in r_nid.finditer(path_str)
            )

    def contacts(self) -> Iterator[NodeID] | None:
        """Returns an interator over all contacts in the routing table."""
        paths_rt = self.paths_routing_table()
        if paths_rt is None:
            return None

        yield from (path[-1] for path in paths_rt)

    def vicinity_graph(self) -> Graph | None:
        """
        Returns the vicinity graph of the node.

        The topology-IDs of the Graph are NodeIDs.
        """

        vicinity_raw = self.api.vicinity_graph()
        if vicinity_raw is None:
            return None

        # Parse neighbor relationships
        node_id_re = re.compile(r"NodeId\((\w+)\)")
        vicinity_graph = Graph()
        entry_re = re.compile(
            r"NodeId\((\w+)\): \[([^]]*?)\]",
            re.DOTALL,  # match newline
        )
        for match in entry_re.finditer(vicinity_raw):
            vid = NodeID.fromhex(match.group(1))
            neighbors = match.group(2)

            neighbor_ids = node_id_re.findall(neighbors)
            for nid in neighbor_ids:
                uid = NodeID.fromhex(nid)
                vicinity_graph.add_edge(vid, uid)

        return vicinity_graph

    def paths_vicinity(self) -> Iterator[Path] | None:
        root = self.node_id  # we are the root of our vicinity graph
        vicinity_graph = self.vicinity_graph()
        if vicinity_graph is None:
            return None

        for v in vicinity_graph:
            if v == root:
                continue

            yield from (
                Path(path[1:])  # exclude root
                for path in nx.all_simple_paths(
                    vicinity_graph, root, v, cutoff=VICINITY_RADIUS
                )
            )

    # FIXME: Only consider nodes in the vicinity graph
    # if they where synced aka have a vicinity_ssn
    def vicinity(self) -> Iterator[NodeID] | None:
        vicinity_graph = self.vicinity_graph()
        if vicinity_graph is None:
            return None
        yield from (node for node in vicinity_graph if node != self.node_id)

    def vicinity_edges(self) -> Iterator[tuple[NodeID, NodeID]] | None:
        vicinity_graph = self.vicinity_graph()
        if vicinity_graph is None:
            return None
        yield from vicinity_graph.edges

    def path(self, of: PathID) -> Path | None:
        """
        Translate a PathIP into its corresponding path of NodeIDs.

        If the node doesn't know the Path to the PathIP None is returned.
        """

        # usually a path used is in some routing table
        # but just to be sure we check the vicinity too
        paths_rt = self.paths_routing_table() or []
        paths_vicinity = self.paths_vicinity() or []
        paths = chain(paths_rt, paths_vicinity)

        for path in paths:
            if of == path.to_path_id():
                return path
        return None

    def missing_vicinity_path_setups(
        self,
    ) -> Iterator[PathIDEntry] | None:
        vicinity_paths = self.paths_vicinity()
        if vicinity_paths is None:
            return None

        for path in vicinity_paths:
            in_path = Path([self.node_id, *path])
            out_path = path

            in_path_ip = in_path.to_path_ip()
            out_path_ip = out_path.to_path_ip()

            if self.next_ip(in_path_ip) is None:
                yield PathIDSwapEntry(in_path, out_path)
            if (
                self.next_hop(out_path_ip) is None
            ):  # TODO: figure out if we do penultimate hop popping (I don't think so)
                yield PathIDFwdEntry(out_path)

    def missing_pathsetups_rt(self) -> Iterator[FwdEntry] | None:
        rt_paths = self.paths_routing_table()
        if rt_paths is None:
            return None

        for path in rt_paths:
            dest = path.destination()
            ulneighbor = len(path) == 1
            dest_ip = dest.to_node_ip()
            next_ip = self.next_ip(dest_ip)

            out_path = path
            out_path_id = path.to_path_id()
            out_path_ip = out_path_id.to_path_ip()

            if ulneighbor:
                # underlay neighbors don't need encapsulation routes
                if next_ip is None:
                    yield NodeIDFwdEntry(dest)
            else:
                if next_ip != out_path_ip:
                    yield NodeIDEncapEntry(dest, out_path)
                if self.next_hop(out_path_ip) is None:
                    yield PathIDFwdEntry(out_path)

    def __format__(self, fmt: str) -> str:
        return f"{self.name:{fmt}}"

    def __str__(self) -> str:
        return self.__format__("")

    def __eq__(self, other: Any) -> bool:
        return other is KIRANode and self.name == other.name

    def __hash__(self) -> int:
        return hash(self.name)
