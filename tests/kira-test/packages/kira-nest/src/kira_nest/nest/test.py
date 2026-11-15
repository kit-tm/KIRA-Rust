import logging
import os
import pathlib
import time
from functools import cached_property
from ipaddress import IPv4Network
from subprocess import Popen

import networkx
from kira_common.domain import (
    ENV_NODE_IP_PREFIX,
    ENV_PATH_IP_PREFIX,
    IPPrefixes,
    KiraIP,
    Path,
    PathIP,
    get_ip_prefixes,
    set_ip_prefixes,
)
from nest.topology import Node, Switch, connect
from nest.topology.interface.interface import create_veth_pair
from networkx import Graph

from kira_nest.domain.topology import KIRATopology
from kira_nest.nest.link import KIRALink
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
    """

    processes = []
    # arguments and wrapper of each node's kirad to start it again
    _daemons: dict = {}

    def __init__(
        self,
        config: Graph | pathlib.Path,
        kirad_binary: pathlib.Path = pathlib.Path("./target/debug/kirad"),
        perf: set[T] | None = None,
        otel_ip: IPv4Network | None = None,
        ip_prefixes: IPPrefixes | None = None,
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

        self.kirad_binary = kirad_binary
        self._daemons = {}
        self._otel_ips = iter(self._otel_ip.hosts())
        next(self._otel_ips)  # skip IP for host

        logger.info("Setting up the topology ...")

        # Create the Nest topology according to the configuration
        for tid, cfg in self.topology.configs():
            node = KIRANode(cfg)
            node.enable_ip_forwarding(True, True)
            self.topology.nodes[tid] = node

        logger.info("Setting up interfaces ...")
        for x, y in self.topology.edges():
            nx = self.topology.nodes[x]
            ny = self.topology.nodes[y]

            if_x, if_y = connect(nx, ny, f"n{x}n{y}", f"n{y}n{x}")
            if_x.set_address(nx.config.ipv6.compressed)
            if_y.set_address(ny.config.ipv6.compressed)

            # safe interfaces for later
            self.topology.links[x, y] = KIRALink(if_x, if_y)

        logger.info("Starting daemons ...")
        for tid, cfg in self.topology.configs():
            node = self.topology.nodes[tid]
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
        kira_process = node.start(self.kirad_binary, *args, wrapper=wrapper, append=True)
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
            case tid:
                try:
                    node = self.topology.nodes[tid]
                    return str(node)
                except KeyError:
                    return "???"

    def _display_path(self, path: Path | None) -> str:
        if path is None:
            return "???"
        else:
            return ", ".join(
                self._display_node(self.topology.nodes[hop]) for hop in path
            )

    def traceroute(  # noqa: PLR0911,PLR0912,PLR0915
        self, start: KIRANode, dest: KIRANode, maxhops: int = 50, verbose: bool = False
    ) -> bool:
        """
        Follows the forwarding tables of the nodes from `start` to `dest`.

        The packet carries a stack of destination IPs: the NodeIP of `dest` at the
        bottom and on top of it the PathIPs of the path segments it is forwarded on.
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
