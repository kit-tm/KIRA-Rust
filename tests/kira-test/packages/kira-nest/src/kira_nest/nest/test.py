import atexit
import contextlib
import logging
import os
import pathlib
import signal
import time
import uuid
from functools import cached_property
from ipaddress import IPv4Network
from subprocess import Popen, TimeoutExpired
from typing import ClassVar, Self

import networkx
from kira_common.domain import (
    ENV_NODE_IP_PREFIX,
    ENV_PATH_IP_PREFIX,
    IPPrefixes,
    KiraIP,
    NodeIP,
    Path,
    PathIP,
    get_ip_prefixes,
    set_ip_prefixes,
)
from nest import engine
from nest.topology import Node, Switch, connect
from nest.topology.id_generator import IdGen
from nest.topology.interface.interface import create_veth_pair
from nest.topology_map import TopologyMap
from networkx import Graph

from kira_nest.domain.topology import KIRATopology
from kira_nest.nest.cluster import Cluster
from kira_nest.nest.link import ForeignLink, KIRALink, VxlanLink
from kira_nest.nest.node import KIRANode

logger = logging.getLogger(__name__)


class KIRATest[T]:  # T = topology id type, usually int or str
    """
    Nest Test
    ===========

    This is a test for the Nest framework.
    It creates a topology and runs a connectivity test between nodes.

    The KIRATest has access to the theoretical topology and therefore
    can test the theoretical view with the one experienced by a KIRANode.

    Parameters
    ----------
    config : Graph or path to GML file
        The configuration for the test.
    ip_prefixes : IPPrefixes, optional
        Prefixes of NodeIPs and PathIPs used by all nodes.
        Defaults to the prefixes from the environment variables also used by kirad.
    cluster : Cluster, optional
        Emulate only the nodes assigned to this host of the cluster and connect them
        to nodes on other hosts over VXLAN (see `kira_nest.nest.cluster`).

    The daemons and namespaces exist until `close()` (also on leaving a `with` block).
    Tests that are still running at interpreter exit are closed then.
    """

    # tests whose daemons may still run
    _running: ClassVar[list["KIRATest"]] = []

    def __init__(  # noqa: PLR0912, PLR0915
        self,
        config: Graph | pathlib.Path,
        kirad_binary: pathlib.Path = pathlib.Path("./target/debug/kirad"),
        perf: set[T] | None = None,
        otel_ip: IPv4Network | None = None,
        ip_prefixes: IPPrefixes | None = None,
        *,
        cluster: Cluster | None = None,
    ) -> None:
        self._otel_ip = otel_ip or IPv4Network("10.42.0.0/24")

        # the checks derive NodeIPs and PathIPs with the same prefixes as the nodes
        if ip_prefixes is not None:
            set_ip_prefixes(ip_prefixes)
        self.ip_prefixes = get_ip_prefixes()
        logger.info(
            f"Using NodeIP prefix {self.ip_prefixes.node_ip}"
            f" and PathIP prefix {self.ip_prefixes.path_ip}"
        )

        if isinstance(config, pathlib.Path):
            # Load the configuration from the GML file
            graph = networkx.readwrite.read_gml(config)
            self.topology = KIRATopology(graph)
        else:
            self.topology = KIRATopology(config)

        self.cluster = cluster
        if cluster is not None:
            cluster.assign(self.topology.topology)
            logger.info(f"Cluster hosts: {cluster.describe()}")

        self.kirad_binary = kirad_binary
        self.processes: list[Popen] = []
        # arguments and wrapper of each node's kirad to start it again
        self._daemons: dict = {}
        KIRATest._running.append(self)
        self._otel_ips = iter(self._otel_ip.hosts())
        next(self._otel_ips)  # skip IP for host

        logger.info("Setting up the topology ...")
        _restart_nest_ids()

        # Create the Nest topology according to the configuration
        for tid, cfg in self.topology.configs():
            if cluster is not None and not cluster.is_local(tid):
                continue
            node = KIRANode(cfg)
            node.enable_ip_forwarding(True, True)
            self.topology.nodes[tid] = node

        logger.info("Setting up interfaces ...")
        for x, y in self.topology.edges():
            if cluster is not None and not (
                cluster.is_local(x) and cluster.is_local(y)
            ):
                continue  # see _setup_cluster_links
            nx = self.topology.nodes[x]
            ny = self.topology.nodes[y]

            if_x, if_y = connect(nx, ny, f"n{x}n{y}", f"n{y}n{x}")
            if_x.set_address(nx.config.ipv6.compressed)
            if_y.set_address(ny.config.ipv6.compressed)

            # safe interfaces for later
            self.topology.links[x, y] = KIRALink(if_x, if_y)
        if cluster is not None:
            self._setup_cluster_links(cluster)

        logger.info("Starting daemons ...")
        for tid, cfg in self.topology.configs():
            node = self.topology.nodes.get(tid)
            if node is None:
                continue  # emulated on another host
            args: list[str] = []

            # kirad inherits the environment, so arguments must override set variables
            if not self.ip_prefixes.is_default() or any(
                env in os.environ for env in (ENV_NODE_IP_PREFIX, ENV_PATH_IP_PREFIX)
            ):
                args.extend(self.ip_prefixes.kirad_args())

            if cfg.otel:
                args.append("--open-telemetry")
                self.setup_otel(node)

            if perf and tid in perf:
                wrapper = (
                    "perf record -F 997 --call-graph dwarf,64000 "
                    f"-g -o log/perf-{node}.data"
                )
            else:
                wrapper = None
            kira_process = node.start(kirad_binary, *args, wrapper=wrapper)
            self.processes.append(kira_process)
            self._daemons[tid] = (args, wrapper)

    def _setup_cluster_links(self, cluster: Cluster) -> None:
        """Sets up the links to nodes on other hosts and between them."""
        graph = self.topology.topology
        cross_links = cluster.cross_links(graph)
        logger.info(f"Setting up {len(cross_links)} VXLAN links to other hosts ...")
        for link in cross_links:
            node = self.topology.nodes[link.local_tid]
            ifname = cluster.create_vxlan(link, node.id, node.config.ipv6.compressed)
            self.topology.links[link.local_tid, link.remote_tid] = VxlanLink(
                node, ifname, link.remote_host.name
            )

        # the theoretical view of the vicinities needs the links beyond this host
        for x, y in graph.edges():
            if not (cluster.is_local(x) or cluster.is_local(y)):
                self.topology.links[x, y] = ForeignLink(
                    cluster.host_of(x), cluster.host_of(y)
                )

    def __enter__(self) -> Self:
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()

    def close(self, timeout: float = 10.0) -> bool:
        """Stops the daemons of all nodes (see `stop_all_daemons`) and deletes the
        namespaces of the test.

        Nest deletes namespaces only at interpreter exit. Namespaces of finished tests
        would pile up during a test session, together with their interfaces, routes
        and eBPF programs, and slow down the host until nodes don't come up in time.
        Can be called more than once.
        """
        stopped = self.stop_all_daemons(timeout)
        self._delete_namespaces()
        with contextlib.suppress(ValueError):
            KIRATest._running.remove(self)
        return stopped

    def _delete_namespaces(self) -> None:
        """Deletes the namespaces of the nodes and of the OTel switch, if created."""
        namespaces = [node.id for node in self.topology.nodes]
        if "_otel_node" in self.__dict__:
            namespaces.append(self._otel_node.id)
        with TopologyMap.lock:
            nodes = TopologyMap.get_nodes()
            for ns_id in namespaces:
                if nodes.pop(ns_id, None) is None:
                    continue  # already deleted
                TopologyMap.devices.pop(ns_id, None)
                engine.delete_ns(ns_id)

    def stop_all_daemons(self, timeout: float = 10.0) -> bool:
        """Stops kirad of all nodes, killing those still running after `timeout`.

        All daemons are signalled at once, so the time doesn't grow with the network.
        Returns if no kirad is running anymore.
        """
        pids = [pid for node in self.topology.nodes for pid in node.kirad_pids()]
        for sig in (signal.SIGTERM, signal.SIGKILL):
            for pid in pids:
                with contextlib.suppress(ProcessLookupError):
                    os.kill(pid, sig)
            deadline = time.monotonic() + timeout
            while (pids := [p for p in pids if _is_running(p)]) and (
                time.monotonic() < deadline
            ):
                time.sleep(0.1)
            if not pids:
                break
            logger.warning(f"{len(pids)} kirad did not stop on {sig.name}")

        # reap the shells that started the daemons
        for p in self.processes:
            with contextlib.suppress(TimeoutExpired):
                p.wait(timeout=1)
        self.processes = [p for p in self.processes if p.returncode is None]
        return not pids

    @classmethod
    def close_all(cls) -> None:
        """Closes all tests that have not been closed."""
        for test in list(cls._running):
            test.close()

    def stop_daemon(self, node: KIRANode, kill: bool = False) -> bool:
        """Stops kirad of the node, with `kill` like on a crash.

        Returns if it is not running anymore.
        """
        return node.stop_kirad(kill=kill)

    def start_daemon(self, node: KIRANode, timeout: float = 10.0) -> bool:
        """Starts kirad of the node again with the same NodeID and arguments.

        Returns if its API is up within `timeout`.
        """
        tid = self.topology.tid(node)
        args, wrapper = self._daemons[tid]
        kira_process = node.start(
            self.kirad_binary, *args, wrapper=wrapper, append=True
        )
        self.processes.append(kira_process)
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if node.is_up():
                return True
            time.sleep(0.1)
        return False

    def setup_otel(self, node: KIRANode) -> None:
        if node.is_up():
            raise NotImplementedError(
                "OpenTelemetry must be enabled before kirad is started"
            )

        otel_ip = next(self._otel_ips)
        assert otel_ip is not None, (
            f"Run out of IPs in {self._otel_ip} to assign to OTel interfaces"
        )

        if_otel, _ = connect(node, self._otel_node, f"{node}notel", f"oteln{node}")

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

        if_otel.set_address(f"{otel_ip.exploded}/{self._otel_ip.prefixlen}")
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

    @cached_property
    def _otel_node(self) -> Node:
        logger.info("Initializing Otel Node ...")

        if all(
            env not in os.environ
            for env in [
                "OTEL_EXPORTER_OTLP_ENDPOINT",
                "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
            ]
        ):
            logger.warning(
                "You haven't set any OTEL_EXPORTER_OTLP*_ENDPOINT environment variable."
                " OpenTelemetry probably won't work."
            )

        otel_node = Switch("OpenTelemetry Collector")
        assert otel_node is not None
        host, otel = create_veth_pair("otel", "otels")

        # move otel interface to otel_node
        otel_node._add_interface(otel)
        otel.set_mode("UP")

        # but leave host interface in default ns for connection
        # since it's not connected to a node we need to invoke `ip` ourselves
        host_ip = (
            f"{next(iter(self._otel_ip.hosts())).exploded}/{self._otel_ip.prefixlen}"
        )
        host = host.id

        Popen(["ip", "address", "add", "dev", host, host_ip])
        Popen(["ip", "link", "set", "dev", host, "up"])

        return otel_node

    def _display_node(self, node: KIRANode | T | None) -> str:
        match node:
            case KIRANode():
                return str(node)
            case None:
                return "???"
            case key:
                # also nodes on other hosts
                return self.topology.display(key)

    def _display_path(self, path: Path | None) -> str:
        if path is None:
            return "???"
        else:
            return ", ".join(self.topology.display(hop) for hop in path)

    def remote_host(self, ip: NodeIP) -> str | None:
        """The host emulating the node with the NodeIP, if it is not this one."""
        if self.cluster is None:
            return None
        try:
            tid = self.topology.topology_nodes[ip].tid
        except KeyError:
            return None
        host = self.cluster.host_of(tid)
        return None if host == self.cluster.local else host

    def traceroute(  # noqa: PLR0911,PLR0912,PLR0915
        self, start: KIRANode, dest: KIRANode, maxhops: int = 50, verbose: bool = False
    ) -> bool | None:
        """
        Follows the forwarding tables of the nodes from `start` to `dest`.

        The packet carries a stack of destination IPs: the NodeIP of `dest` at the
        bottom and on top of it the PathIPs of the path segments it is forwarded on.

        Returns None if the packet is forwarded to a node on another host, whose
        forwarding tables can't be followed from here.
        """
        current_hop = start
        current_ip = current_hop.node_id.to_node_ip()

        dst_hop = dest
        dst_ip = dst_hop.node_id.to_node_ip()
        stack: list[KiraIP] = [dst_ip]

        def log(action: str) -> None:
            if verbose:
                print(f"{current_hop:<3} : {action}")

        if verbose:
            print(f"Tracerouting from {current_hop} to {dst_ip} ({dst_hop}):")

        hc = 0
        while hc <= maxhops:
            # process the stack until the packet leaves the node
            while True:
                top = stack[-1]
                if isinstance(top, PathIP):
                    # PathIPs on arriving packets are swapped or popped
                    next_ip = current_hop.next_ip(top)
                    if next_ip is None:
                        log(f"ERR unknown  : {top}")
                        return False
                    if next_ip == current_ip:
                        # end of the path segment
                        stack.pop()
                        log(f"POP  Path-ID : {top} --> {stack[-1]}")
                        continue
                    path_str = self._display_path(
                        current_hop.path(next_ip.to_path_id())  # type: ignore -- PathIP
                    )
                    log(f"SWAP Path-ID : {top} --> {next_ip} ({path_str})")
                    stack[-1] = next_ip
                    break

                if top == current_ip:
                    if len(stack) == 1:
                        log("ACK")
                        return True
                    log(f"ERR : unexpected encapsulation of own NodeIP: {stack}")
                    return False

                # overlay hop: encapsulate with the PathIPs of the path segments
                labels = current_hop.encap_labels(top)
                if labels is None:
                    log(f"ERR unknown  : {top}")
                    return False
                for label in reversed(labels):
                    path_str = self._display_path(current_hop.path(label.to_path_id()))
                    log(f"PUSH Path-ID : {label} ({path_str})")
                    stack.append(label)
                break

            next_ip = current_hop.next_hop(stack[-1])
            if next_ip is None:
                print(f"{current_hop:<3} : ERR : Can't determine next ip")
                return False
            next_hop = self.topology.nodes.get(next_ip)
            if next_hop is None and (host := self.remote_host(next_ip)) is not None:
                log(
                    f"FORWARD to {self.topology.display(next_ip)} on host {host},"
                    " not traced further"
                )
                return None
            if next_hop is None:
                print(
                    f"{current_hop:<3} : ERR :"
                    f"Can't determine next hop based on IPv6: {next_ip}"
                )
                return False

            if current_hop != next_hop:
                hc += 1
                log(f"FORWARD to {next_hop}")

            current_hop = next_hop
            current_ip = next_ip

        # maxhop limit reached
        if verbose:
            print(f"{current_hop:<3} : HLIMIT = {maxhops} reached")
        return False


def _is_running(pid: int) -> bool:
    """Returns if the process exists and is not a zombie."""
    try:
        stat = pathlib.Path(f"/proc/{pid}/stat").read_text()
    except OSError:
        return False
    # the state follows the command name, which is in parentheses
    return stat.rpartition(")")[2].split()[0] != "Z"


def _restart_nest_ids() -> None:
    """Lets Nest number the namespaces and interfaces of a new test from the start.

    Nest names them `<10 hex digits>-<counter>` with one counter for the whole process.
    From counter 10000 on, the names exceed the 15 characters Linux allows for
    interface names, and Nest fails silently to create the interfaces. A fresh prefix
    keeps the names unique, also next to the namespaces of tests still running.
    """
    with IdGen.lock:
        IdGen.topology_id = uuid.uuid4().hex[:10]
        IdGen.counter = 0


# Nest registered the deletion of the namespaces when it was imported (above). Exit
# handlers run in reverse order, so this one runs first, while `ip netns pids` still
# finds the daemons.
atexit.register(KIRATest.close_all)
