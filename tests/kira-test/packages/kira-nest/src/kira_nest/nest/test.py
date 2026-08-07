import logging
import os
import pathlib
from functools import cached_property
from ipaddress import IPv4Network
from subprocess import Popen
from typing import overload

import networkx
from kira_common.domain import NodeIP, Path, PathIP
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
    """

    processes = []

    def __init__(
        self,
        config: Graph | pathlib.Path,
        kirad_binary: pathlib.Path = pathlib.Path("./target/debug/kirad"),
        perf: set[T] | None = None,
        otel_ip: IPv4Network | None = None,
    ) -> None:
        self._otel_ip = otel_ip or IPv4Network("10.42.0.0/24")

        if isinstance(config, pathlib.Path):
            # Load the configuration from the GML file
            graph = networkx.readwrite.read_gml(config)
            self.topology = KIRATopology(graph)
        else:
            self.topology = KIRATopology(config)

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

            if cfg.otel:
                args.append("--open-telemetry")
                self.setup_otel(node)

            if perf and tid in perf:
                wrapper = f"perf record -F 997 --call-graph dwarf,64000 -g -o log/perf-{node}.data"
            else:
                wrapper = None
            kira_process = node.start(kirad_binary, wrapper=wrapper, *args)
            self.processes.append(kira_process)

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

    @overload
    def _describe_hop_action(
        self,
        current_hop: KIRANode,
        from_addr: NodeIP,
        to_addr: PathIP,
        path: Path | None,
    ) -> str: ...

    @overload
    def _describe_hop_action(
        self,
        current_hop: KIRANode,
        from_addr: PathIP,
        to_addr: NodeIP,
        path: Path | None,
    ) -> str: ...

    @overload
    def _describe_hop_action(
        self,
        current_hop: KIRANode,
        from_addr: PathIP,
        to_addr: PathIP,
        path: Path | None,
    ) -> str: ...

    def _describe_hop_action(
        self,
        current_hop,
        from_addr,
        to_addr,
        path,
    ) -> str:
        path_str = self._display_path(path)

        # print action that lead to label change
        if isinstance(from_addr, PathIP) and isinstance(to_addr, PathIP):
            return f"{current_hop:<3} : SWAP Path-ID : {from_addr} --> {to_addr} ({path_str})"  # noqa: E501
        elif isinstance(from_addr, PathIP):
            return f"{current_hop:<3} : POP  Path-ID : {from_addr} --> {to_addr}"
        elif isinstance(to_addr, PathIP):
            return f"{current_hop:<3} : PUSH Path-ID : {to_addr} ({path_str})"
        else:
            return "??? Unknown Action ???"

    def traceroute(  # noqa: PLR0912
        self, start: KIRANode, dest: KIRANode, maxhops: int = 50, verbose: bool = False
    ) -> bool:
        current_hop = start
        current_ip = current_hop.node_id.to_node_ip()

        dst_hop = dest
        dst_ip = dst_hop.node_id.to_node_ip()
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
                if isinstance(prev_outer_ip, NodeIP) and isinstance(outer_ip, PathIP):
                    current_path = current_hop.path(outer_ip.to_path_id())
                elif current_path is not None:
                    if len(current_path) == 1:
                        current_path = None  # path is finished
                    else:
                        current_path = Path(current_path[1:])  # advance path

                # don't change intended destination!
                assert isinstance(prev_outer_ip, PathIP) or isinstance(outer_ip, PathIP)

                # print action that lead to label change
                if verbose:
                    descr = self._describe_hop_action(
                        current_hop,
                        prev_outer_ip,  # type: ignore -- guaranteed by assertion
                        outer_ip,  # type: ignore -- guaranteed by assertion
                        current_path,
                    )
                    print(descr)

            next_ip = current_hop.next_hop(outer_ip)
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
