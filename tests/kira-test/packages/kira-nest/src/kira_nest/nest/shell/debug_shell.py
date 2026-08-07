from __future__ import annotations

import argparse
import hashlib
import re
import socket
import sys
import time
from cmd import Cmd
from collections.abc import Iterable
from functools import cached_property
from pathlib import Path
from subprocess import PIPE, Popen
from typing import Any

from kira_common.domain import NodeID, NodeIP, PathIP
from kira_common.domain.forwarding import (
    FwdEntry,
    NodeIDEncapEntry,
    NodeIDFwdEntry,
    PathIDFwdEntry,
    PathIDSwapEntry,
)
from nest.topology.address import Address
from PIL import Image
from term_image.image import AutoImage

from kira_nest.nest.imager import KIRAImager
from kira_nest.nest.link import KIRALink
from kira_nest.nest.node import KIRANode
from kira_nest.nest.shell.util import (
    StoreKIRAIP,
    StoreNode,
    init_argparser,
    with_argparser,
)
from kira_nest.nest.test import KIRATest

RED = "\x01\033[91m\x02"
GREEN = "\x01\033[92m\x02"
YELLOW = "\x01\033[93m\x02"
BLUE = "\x01\033[94m\x02"

BOLD = "\x01\033[1m\x02"
RESET = "\x01\033[0m\x02"


@init_argparser
class DebugShell[T](Cmd):
    intro = (
        "Welcome to the debug shell of NeST-Test. "
        "Type help or ? to list commands, exit to quit.\n"
    )
    file = None
    failure = False  # a command failed
    exit_on_failure = False  # set -e
    print_cmd = False  # set -x
    quiet = False

    def __init__(
        self,
        test: KIRATest[T],
        unshared: bool = False,
        post_process_log_files: bool = False,
    ) -> None:
        super().__init__()
        self.test = test
        self.imager = KIRAImager(test)
        self.post_process_log_files = post_process_log_files

        base = f"{YELLOW}ntest{RESET}"
        prefix = (
            f"{GREEN}unshared{RESET}"
            if unshared
            else f"{RED}{socket.gethostname()}{RESET}"
        )
        self.prompt = f"{base}{BLUE}@{RESET}{prefix} {BOLD}${RESET} "

    @cached_property
    def _replacement_map(self) -> dict[str, str]:
        replacement_map = {}
        for tid, node_cfg in self.test.topology.configs():
            nid = node_cfg.node_id
            ipv6 = node_cfg.ipv6
            short_nid = nid[:8]
            replacement = f"${tid}$"

            replacement_map[nid] = f"{replacement:<{len(nid)}}"
            replacement_map[short_nid] = f"{replacement:<{len(short_nid)}}"
            replacement_map[ipv6] = f"{replacement:<{len(ipv6)}}"
        return replacement_map

    @cached_property
    def _replace_re(self) -> re.Pattern:
        replace_re = "|".join(re.escape(nid) for nid in self._replacement_map)
        ignore_case = f"(?i:{replace_re})"
        return re.compile(ignore_case)

    def _cmd_failed(self) -> None:
        self.failure = True

    def sub_nid_name(self, string: str) -> str:
        """
        Substitute Node-IDs with the corresponding name used in the topology.

        Shortened Node-IDs of length 8
        and the IPv6-addresses of the nodes are also replaced.
        """

        def replace(match: re.Match) -> str:
            matched: str = match.group(0).lower()
            replace_with = self._replacement_map.get(matched, matched)
            return replace_with

        return self._replace_re.sub(replace, string)

    def process_log_file(
        self, input_path: Path, output_path: Path | None = None, encoding: str = "utf-8"
    ) -> Path:
        input_file = Path(input_path)
        output_file = Path(output_path) if output_path else input_file

        # Use a temporary file necessary on overwriting
        temp_file = output_file.with_suffix(output_file.suffix + ".tmp")
        with (
            input_file.open(encoding=encoding, errors="replace") as infile,
            temp_file.open("w", encoding=encoding) as outfile,
        ):
            for line in infile:
                processed_line = self.sub_nid_name(line)
                outfile.write(processed_line)
        temp_file.replace(output_file)
        return output_file

    _pingall_parser = argparse.ArgumentParser()
    _pingall_parser.add_argument(
        "-f", "--failed", action="store_true", help="display only failed pings"
    )
    _pingall_parser.add_argument(
        "-v", "--verbose", action="store_true", help="more verbose pings"
    )

    # TODO: support pinging specific and partial node pairs
    @with_argparser(_pingall_parser)
    def do_pingall(self, args: argparse.Namespace) -> None:
        failed = args.failed or self.quiet
        verbose = 2 if args.verbose else 0

        conn = dict(self.test.topology.connected_components())
        for x in self.test.topology.nodes:
            for y in self.test.topology.nodes:
                if x == y or conn[x] != conn[y]:
                    continue

                if not self.quiet:
                    print(f"Pinging {x:>3} --> {y:>3} ...", end="\r")
                ip_y = y.node_id.to_node_ip()
                ip_y = Address(str(ip_y))
                ping_failed = not x.ping(ip_y, packets=1, verbose=verbose)

                if verbose == 0:
                    if ping_failed:
                        print(f"Pinging {x:>3} --> {y:>3} ✗   ", flush=True)
                        self._cmd_failed()
                        continue

                    if not self.quiet:
                        # overwrite line if failed
                        end = "\r" if failed else "\n"
                        print(f"Pinging {x:>3} --> {y:>3} ✓  ", end=end, flush=True)

    @property
    def _exec_parser(self) -> argparse.ArgumentParser:
        parser = argparse.ArgumentParser(
            description="Execute arbitrary command in the network namespace of a node."
        )
        parser.add_argument(
            "node",
            action=StoreNode,
            node_view=self.test.topology.nodes,
            help="identifier of the node (name, NodeId, topology id)",
        )
        parser.add_argument(
            "cmd",
            help="command that is executed in the namespace of the node",
        )
        return parser

    @with_argparser("_exec_parser")
    def do_exec(self, args: argparse.Namespace) -> None:
        node: KIRANode = args.node
        cmd = args.cmd

        p = node.exec(cmd, logfile=sys.stdout)
        exit_code = p.wait()
        if exit_code != 0:
            self._cmd_failed()
        print()

    @property
    def _api_parser(self) -> argparse.ArgumentParser:
        parser = argparse.ArgumentParser(
            description="Issue arbitrary API call to a node."
        )
        parser.add_argument(
            "node",
            action=StoreNode,
            node_view=self.test.topology.nodes,
            help="identifier of the node (name, NodeId, topology id)",
        )
        parser.add_argument(
            "cmd",
            help="REST-API path",
        )
        return parser

    @with_argparser("_api_parser")
    def do_api(self, args: argparse.Namespace) -> None:
        node: KIRANode = args.node
        path = args.path

        res = node.api.call(path)
        if res is None:
            print("ERR: API call failed")
            self._cmd_failed()
            return
        res = self.sub_nid_name(res)
        print(res)

    @property
    def _node_id_parser(self) -> argparse.ArgumentParser:
        parser = argparse.ArgumentParser(description="Get the Node-ID of a node.")
        parser.add_argument(
            "node",
            action=StoreNode,
            node_view=self.test.topology.nodes,
            help="identifier of the node (name, NodeId, topology id)",
        )
        return parser

    @with_argparser("_node_id_parser")
    def do_node_id(self, args: argparse.Namespace) -> None:
        node: KIRANode = args.node

        node_id = node.api.node_id()
        if node_id is None:
            print("ERR: Unable to reach KIRA API backend of node.")
        else:
            print(node_id)

    @property
    def _store_parser(self) -> argparse.ArgumentParser:
        parser = argparse.ArgumentParser(
            description="Store a key-value pair in the Distributed Hash Table."
        )
        parser.add_argument(
            "node",
            action=StoreNode,
            node_view=self.test.topology.nodes,
            help="identifier of the node to store from",
        )
        parser.add_argument(
            "key",
            help="Key",
        )
        parser.add_argument(
            "value",
            help="Value",
        )
        return parser

    @with_argparser("_store_parser")
    def do_store(self, args: argparse.Namespace) -> None:
        node: KIRANode = args.node
        # TODO: parse result of store to catch failure
        print(node.api.store(args.key, args.value))

    @property
    def _fetch_parser(self) -> argparse.ArgumentParser:
        parser = argparse.ArgumentParser(
            description="Obtain the value of a key in the Distributed Hash Table."
        )
        parser.add_argument(
            "node",
            action=StoreNode,
            node_view=self.test.topology.nodes,
            help="identifier of the node to start the fetch query",
        )
        parser.add_argument(
            "key",
            help="Key",
        )
        return parser

    @with_argparser("_fetch_parser")
    def do_fetch(self, args: argparse.Namespace) -> None:
        node: KIRANode = args.node
        key = args.key

        res = node.api.fetch(key)
        if len(res) == 0:
            print(f"No value with key {key} found!")
            self._cmd_failed()
        elif len(res) == 1:
            print(f"{key}={res[0]}")
        else:
            print(f"{key}=[")
            for value in res:
                print(f"    {value},")
            print("]")

    @property
    def _routing_table_parser(self) -> argparse.ArgumentParser:
        parser = argparse.ArgumentParser(
            description="Dumps the routing table of a node."
        )
        parser.add_argument(
            "node",
            action=StoreNode,
            node_view=self.test.topology.nodes,
            help="identifier of a node",
        )
        return parser

    @with_argparser("_routing_table_parser")
    def do_routing_table(self, args: argparse.Namespace) -> None:
        node: KIRANode = args.node

        res = node.api.routing_table()
        if res is None:
            print("ERR: API call failed")
            self._cmd_failed()
            return
        res = self.sub_nid_name(res)
        print(res)

    @property
    def _contacts_parser(self) -> argparse.ArgumentParser:
        parser = argparse.ArgumentParser(
            description="Contacts and their paths of a node."
        )
        parser.add_argument(
            "node",
            action=StoreNode,
            node_view=self.test.topology.nodes,
            help="identifier of a node",
        )
        return parser

    @with_argparser("_contacts_parser")
    def do_contacts(self, args: argparse.Namespace) -> None:
        node: KIRANode = args.node

        for path in node.paths_routing_table() or []:
            contact = self.test.topology.nodes.get(path[-1]) or "???"
            path_str = self.test._display_path(path)

            print(f"{contact:>3} ==> {path_str}")

    @property
    def _uln_table_parser(self) -> argparse.ArgumentParser:
        parser = argparse.ArgumentParser(
            description="Dumps the underlay neighbor table of a node."
        )
        parser.add_argument(
            "node",
            action=StoreNode,
            node_view=self.test.topology.nodes,
            help="identifier of a node",
        )
        return parser

    @with_argparser("_uln_table_parser")
    def do_uln_table(self, args: argparse.Namespace) -> None:
        node: KIRANode = args.node

        res = node.api.uln_table()
        if res is None:
            print("ERR: API call failed")
            self._cmd_failed()
            return
        res = self.sub_nid_name(res)
        print(res)

    @property
    def _vicinity_graph_parser(self) -> argparse.ArgumentParser:
        parser = argparse.ArgumentParser(
            description="Dumps the vicinity graph of a node."
        )
        parser.add_argument(
            "node",
            action=StoreNode,
            node_view=self.test.topology.nodes,
            help="identifier of a node",
        )
        return parser

    @with_argparser("_vicinity_graph_parser")
    def do_vicinity_graph(self, args: argparse.Namespace) -> None:
        node: KIRANode = args.node

        res = node.api.vicinity_graph()
        if res is None:
            print("ERR: API call failed")
            self._cmd_failed()
            return
        res = self.sub_nid_name(res)
        print(res)

    @property
    def _local_hashtable_parser(self) -> argparse.ArgumentParser:
        parser = argparse.ArgumentParser(
            description="Dumps the local hashtable (part of the DHT) of a node."
        )
        parser.add_argument(
            "node",
            action=StoreNode,
            node_view=self.test.topology.nodes,
            help="identifier of a node",
        )
        return parser

    @with_argparser("_local_hashtable_parser")
    def do_local_hashtable(self, args: argparse.Namespace) -> None:
        node: KIRANode = args.node

        res = node.api.local_hashtable()
        if res is None:
            print("ERR: API call failed")
            self._cmd_failed()
            return
        print(res)

    @property
    def _checkup_parser(self) -> argparse.ArgumentParser:
        parser = argparse.ArgumentParser(description="Check if node(s) are up.")
        parser.add_argument(
            "node",
            nargs="?",
            action=StoreNode,
            node_view=self.test.topology.nodes,
            help="identifier of a node",
        )
        return parser

    @with_argparser("_checkup_parser")
    def do_checkup(self, args: argparse.Namespace) -> None:
        # check all if no node is specified
        if args.node is None:
            all_up = True
            for n in self.test.topology.nodes:
                if n.is_down():
                    print(f"Node {n} is down.")
                    all_up = False
                    self._cmd_failed()
            if all_up:
                print("All nodes are up!")

            return

        node: KIRANode = args.node
        if node.is_up():
            print(f"Node {node} is up")
        else:
            print(f"Node {node} is not up")
            self._cmd_failed()

    @property
    def _next_ip_parser(self) -> argparse.ArgumentParser:
        parser = argparse.ArgumentParser()
        parser.add_argument(
            "node",
            nargs="?",
            action=StoreNode,
            node_view=self.test.topology.nodes,
            help="identifier of a node",
        )
        parser.add_argument(
            "kira_ip",
            action=StoreKIRAIP,
            help="lookup address",
        )
        return parser

    @with_argparser("_next_ip_parser")
    def do_next_ip(self, args: argparse.Namespace) -> None:
        node = args.node
        kira_ip = args.kira_ip

        next_ip = node.next_ip(kira_ip)
        print()
        print(next_ip)
        if next_ip is None:
            self._cmd_failed()

    @property
    def _next_hop_parser(self) -> argparse.ArgumentParser:
        parser = argparse.ArgumentParser()
        parser.add_argument(
            "node",
            nargs="?",
            action=StoreNode,
            node_view=self.test.topology.nodes,
            help="identifier of a node",
        )
        parser.add_argument(
            "kira_ip",
            action=StoreKIRAIP,
            help="lookup address",
        )
        return parser

    @with_argparser("_next_hop_parser")
    def do_next_hop(self, args: argparse.Namespace) -> None:
        node = args.node
        kira_ip = args.kira_ip

        next_hop = node.next_hop(kira_ip)
        print()
        print(next_hop)
        if next_hop is None:
            self._cmd_failed()

    @property
    def _path_parser(self) -> argparse.ArgumentParser:
        parser = argparse.ArgumentParser(description="Lookup Path-ID on the node.")
        parser.add_argument(
            "node",
            action=StoreNode,
            node_view=self.test.topology.nodes,
            help="identifier of a node",
        )
        parser.add_argument(
            "path_ip",
            nargs="?",
            type=PathIP,
            help="lookup address",
        )
        return parser

    @with_argparser("_path_parser")
    def do_path(self, args: argparse.Namespace) -> None:
        node = args.node
        ip = args.path_ip

        if ip is None:
            paths = node.paths_routing_table()
            if paths is None:
                self._cmd_failed()
                return
        else:
            path = node.path(ip.to_path_id())
            if path is None:
                print(f"ERR: Unknown PathIP: {ip}")
                self._cmd_failed()
                return
            paths = [path]

        for path in paths:
            path_str = self.test._display_path(path)
            path_ip = path.to_path_ip()
            print(f"{path_ip} ==> {path_str}")

    @property
    def _traceroute_parser(self) -> argparse.ArgumentParser:
        parser = argparse.ArgumentParser(
            description="Simulate traceroutes of fast-forwarding paths."
        )
        parser.add_argument(
            "source",
            nargs="?",
            action=StoreNode,
            node_view=self.test.topology.nodes,
            help="source node",
        )
        parser.add_argument(
            "destination",
            nargs="?",
            action=StoreNode,
            node_view=self.test.topology.nodes,
            help="destination node",
        )
        parser.add_argument(
            "-v", "--verbose", action="store_true", help="print successful traceroutes"
        )
        parser.add_argument(
            "-f", "--failed", action="store_true", help="display only failed traces"
        )

        return parser

    @with_argparser("_traceroute_parser")
    def do_traceroute(self, args: argparse.Namespace) -> None:
        src = args.source
        dst = args.destination

        specific_trace = src is not None and dst is not None
        srcs = [src] if src is not None else self.test.topology.nodes
        dsts = [dst] if dst is not None else self.test.topology.nodes
        conn = dict(self.test.topology.connected_components())

        for src in srcs:
            for dst in dsts:
                if conn[src] != conn[dst] and not specific_trace:
                    continue

                if not self.quiet and not args.verbose:
                    print(f"{src:>3} --> {dst:>3} ...", end="\r")
                success = self.test.traceroute(src, dst, verbose=args.verbose)
                end = "\r" if args.failed and success else "\n"
                success = "✓   " if success else "✗   "
                print(f"{src:>3} --> {dst:>3} {success}", end=end, flush=True)

                # only show unsuccessful traceroutes
                if not success and not args.verbose:
                    self._cmd_failed()
                    self.test.traceroute(src, dst, verbose=True)
                    print()
                    print("==============================================")
                    print()

    @property
    def _link_parser(self) -> argparse.ArgumentParser:
        def parse_up_down(value: str) -> bool:
            mode = value.lower()
            if "up".startswith(mode):
                return True
            if "down".startswith(mode):
                return False

            raise ValueError(f"Invalid value '{value}'. Must be 'u[p]' or 'd[own]'.")

        parser = argparse.ArgumentParser(description="Sets link up or down.")
        parser.add_argument(
            "mode",
            type=parse_up_down,
            help="Sets the link to 'mode'. Either 'u[p]' or 'd[own]'.",
        )
        parser.add_argument(
            "node_x",
            action=StoreNode,
            node_view=self.test.topology.nodes,
        )
        parser.add_argument(
            "node_y",
            nargs="?",
            action=StoreNode,
            node_view=self.test.topology.nodes,
        )

        return parser

    @with_argparser("_link_parser")
    def do_link(self, args: argparse.Namespace) -> None:
        node_x = args.node_x
        node_y = args.node_y
        mode = args.mode

        x_tid = self.test.topology.tid(node_x)
        if node_y is not None:
            y_tid = self.test.topology.tid(node_y)
            assert y_tid is not None
            ys_tid = [y_tid]
        else:
            # all links from x
            ys_tid = [y_tid for y_tid, _ in self.test.topology.links[x_tid, ...]]

        if mode:
            mode_sym = "✔"
            mode_fn = KIRALink.up
        else:
            mode_sym = "✗"
            mode_fn = KIRALink.down

        for y_tid in ys_tid:
            node_y = self.test.topology.nodes[y_tid]
            link = self.test.topology.links[x_tid, y_tid]
            assert isinstance(link, KIRALink)
            mode_fn(link)

            if not self.quiet:
                print(f"{node_x:>3} -{mode_sym}- {node_y:>3} ...")

    @property
    def _down_parser(self) -> argparse.ArgumentParser:
        parser = argparse.ArgumentParser(description="Sets link down.")
        parser.add_argument(
            "node_x",
            action=StoreNode,
            node_view=self.test.topology.nodes,
        )
        parser.add_argument(
            "node_y",
            nargs="?",
            action=StoreNode,
            node_view=self.test.topology.nodes,
        )

        return parser

    @with_argparser("_down_parser")
    def do_down(self, args: argparse.Namespace) -> None:
        args.mode = False
        self.do_link(args)

    @property
    def _up_parser(self) -> argparse.ArgumentParser:
        parser = argparse.ArgumentParser(description="Sets link up.")
        parser.add_argument(
            "node_x",
            action=StoreNode,
            node_view=self.test.topology.nodes,
        )
        parser.add_argument(
            "node_y",
            nargs="?",
            action=StoreNode,
            node_view=self.test.topology.nodes,
        )

        return parser

    @with_argparser("_up_parser")
    def do_up(self, args: argparse.Namespace) -> None:
        args.mode = True
        self.do_link(args)

    @property
    def _links_parser(self) -> argparse.ArgumentParser:
        parser = argparse.ArgumentParser(
            description="List links in the topology and their status."
        )
        parser.add_argument(
            "node",
            nargs="?",
            action=StoreNode,
            node_view=self.test.topology.nodes,
        )
        return parser

    @with_argparser("_links_parser")
    def do_links(self, args: argparse.Namespace) -> None:
        node = args.node

        if node is None:
            for x_tid, y_tid, link in self.test.topology.links:
                x = self.test.topology.nodes[x_tid]
                y = self.test.topology.nodes[y_tid]
                up_indicator = "-" if link.is_up() else "✗"
                print(f"{x:>3} -{up_indicator}- {y:>3}")
        else:
            x = node
            x_tid = self.test.topology.tid(x)
            for y_tid, link in self.test.topology.links[x_tid, ...]:
                y = self.test.topology.nodes[y_tid]
                up_indicator = "-" if link.is_up() else "✗"
                print(f"{x:>3} -{up_indicator}- {y:>3}")

    @property
    def _edge_parser(self) -> argparse.ArgumentParser:
        parser = self._link_parser
        parser.description = f"{parser.description} Alias for 'links'."
        return parser

    @with_argparser("_edge_parser")
    def do_edges(self, args: argparse.Namespace) -> None:
        self.do_links(args)

    _nodes_parser = argparse.ArgumentParser(
        description="List all nodes present in the topology."
    )

    @property
    def _pcap_parser(self) -> argparse.ArgumentParser:
        parser = argparse.ArgumentParser(
            formatter_class=argparse.RawDescriptionHelpFormatter,
            description="""\
            Capture packets sent over links of the topology.

            The packets are always captured in the network-namespace of `node_x`.
            If you omit `node_y` all links of `node_x` will be captured.

            Abort packet capture using `pkill -SIGHUP tcpdump`.

            You should use a named pipe for package capture if you want to
            watch the packets captured in real-time.
            You can create a named pipe using `mkfifo` and later watch the
            captured packets live using `wireshark -k -i <FIFO>`.
            The capture is aborted as soon as you stop the package capture in
            Wireshark.
        """,
        )
        parser.add_argument(
            "node_x",
            action=StoreNode,
            node_view=self.test.topology.nodes,
        )
        parser.add_argument(
            "node_y",
            nargs="?",
            action=StoreNode,
            node_view=self.test.topology.nodes,
        )
        parser.add_argument(
            "pcap",
            type=Path,
        )
        return parser

    @with_argparser("_pcap_parser")
    def do_pcap(self, args: argparse.Namespace) -> None:
        node_x: KIRANode = args.node_x
        node_y = args.node_y
        output_file = args.pcap

        x_tid = self.test.topology.tid(node_x)
        if node_y is not None:
            y_tid = self.test.topology.tid(node_y)
            assert y_tid is not None
            ys_tid = [y_tid]
        else:
            # capture all links of x
            ys_tid = [y_tid for y_tid, _ in self.test.topology.links[x_tid, ...]]

        interfaces_str = ""
        for y_tid in ys_tid:
            node_y = self.test.topology.nodes[y_tid]
            link = self.test.topology.links[x_tid, y_tid]
            assert isinstance(link, KIRALink)
            interface = link.id(node_x)
            if interface is None:
                print(f"Can't determine interface of {node_x} to {node_y}")
            else:
                interfaces_str += f"-i {interface} "
        node_x.exec(f"tcpdump {interfaces_str} -w {output_file}")

    @with_argparser(_nodes_parser)
    def do_nodes(self, args: argparse.Namespace) -> None:
        _ = args

        for n in self.test.topology.nodes:
            nid = n.node_id
            print(f"{n:>3} {nid}")

    def do_show_graph(self, arg: str) -> None:
        "Show the current graph of the topology."
        _ = arg

        buffer = self.imager.image_topology(dpi=400)
        img = Image.open(buffer)
        self.current_image = AutoImage(img)
        self.current_image.draw()

    @property
    def _vicinity_parser(self) -> argparse.ArgumentParser:
        parser = argparse.ArgumentParser(
            description="Get information of the node vicinity."
        )
        parser.add_argument(
            "node",
            action=StoreNode,
            node_view=self.test.topology.nodes,
        )
        return parser

    @with_argparser("_vicinity_parser")
    def do_vicinity(self, args: argparse.Namespace) -> None:  # noqa: PLR0915, PLR0912
        node = args.node
        tid = self.test.topology.tid(node)
        topo_node = self.test.topology.topology_nodes[tid]

        # sort topo-ids like `k17` in expected order
        def _srt(n: KIRANode) -> tuple[str | KIRANode, int]:
            match = re.match(r"([a-zA-Z]+)(\d+)", n.name)
            if match:
                alpha, num = match.groups()
                return (alpha, int(num))
            return (n, 0)

        def _sorted(iiter: Iterable[T | NodeID | NodeIP | str]) -> Iterable[KIRANode]:
            return sorted(map(lambda t: self.test.topology.nodes[t], iiter), key=_srt)

        if not self.quiet:
            print("Theoretical Vicinity:")
            buffer = self.imager.image_vicinity(topo_node.tid, dpi=300)
            img = Image.open(buffer)
            img = AutoImage(img)
            img.set_size(height=20)
            img.draw(h_align="left", v_align="top", pad_height=1)

        if not self.quiet:
            print("Vicinity Status:")
            buffer = self.imager.image_node(node, dpi=300)
            img = Image.open(buffer)
            self.current_image = AutoImage(img)
            self.current_image.set_size(height=20)
            self.current_image.draw(h_align="left", v_align="top", pad_height=1)

        unknowns = topo_node.unknown_vicinity(node)
        unknowns = set(unknowns)
        if len(unknowns) != 0:
            dvicinity = node.vicinity()
            print("Discovered Vicinity:")
            if dvicinity is None:
                print("ERR: determining known vicinity.")
                self._cmd_failed()
            else:
                for v in _sorted(set(dvicinity)):
                    print(f"{v:>3}")
                print()

            print("Missing Nodes in the Vicinity Graph:")
            for unknown in _sorted(unknowns):
                print(f"{unknown:>3}")
                self._cmd_failed()
        else:
            print("All vicinity nodes where discoverd.")
        print()

        unknown_edges = topo_node.unknown_vicinity_edges(node)
        unknown_edges = set(unknown_edges)
        if len(unknown_edges) != 0:
            print("Unknown Vicinity Edges:")
            self._cmd_failed()
            for u, v in unknown_edges:
                print(f"{u:>3} -?- {v:>3}")
        else:
            print("All edges in the vicinity where discovered.")
        print()

        def _print_fwd_entry(entry: FwdEntry) -> None:
            match entry:
                case NodeIDFwdEntry(n):
                    # fe80::/64 is indicative that forwarding is done using some LL-IP
                    # TODO: print actual LL-IP used
                    print(f"{n.to_node_ip()}/16 -> fe80::/64")
                case NodeIDEncapEntry(n, path):
                    path_ip = path.to_path_ip()
                    path = self.test._display_path(path)  # replace NID with node names

                    print(f"{n.to_node_ip()}/16 -> {path_ip}/16 [ {path} ]")
                case PathIDFwdEntry(path):
                    path_ip = path.to_path_ip()
                    neighbor_ip = path[0].to_node_ip()
                    path = self.test._display_path(path)

                    print(f"{path_ip}/16 -> {neighbor_ip}/16 [ {path} ]")
                case PathIDSwapEntry(in_path, out_path):
                    in_path_ip = in_path.to_path_ip()
                    out_path_ip = out_path.to_path_ip()
                    in_path = self.test._display_path(in_path)
                    out_path = self.test._display_path(out_path)

                    print(
                        f"{in_path_ip}/16 -> {out_path_ip}/16 "
                        f"[ {in_path} -> {out_path} ]"
                    )

        missing_entries = node.missing_vicinity_path_setups()
        if missing_entries is not None:
            missing_entries = list(missing_entries)
            if len(missing_entries) != 0:
                self._cmd_failed()
                print("Missing path setups:")
                for entry in missing_entries:
                    print("  - ", end="")
                    _print_fwd_entry(entry)
            else:
                print("All paths inside the vicinity are setup.")
        else:
            print("ERR: checking on missing path setups.")
            self._cmd_failed()
        print()

        missing_fwd_entries = node.missing_pathsetups_rt()
        if missing_fwd_entries is not None:
            missing_fwd_entries = list(missing_fwd_entries)
            if len(missing_fwd_entries) != 0:
                self._cmd_failed()
                print("Missing paths or nodes setups of Contacts in the Routing Table:")
                for entry in missing_fwd_entries:
                    print("  - ", end="")
                    _print_fwd_entry(entry)

            else:
                print("All paths and nodes of Contacts in the Routing Table are setup.")
        else:
            print("ERR: checking on missing path setups.")
            self._cmd_failed()
        print()

        additional_vicinity = topo_node.additional_vicinity(node)
        additional_vicinity = set(additional_vicinity)
        if len(additional_vicinity) != 0:
            self._cmd_failed()
            print("Additional Vicinity:")
            for additional in _sorted(additional_vicinity):
                print(f"{additional:>3}")
        else:
            print("No unexpected nodes in the vicinity.")
        print()

    @property
    def _check_parser(self) -> argparse.ArgumentParser:
        parser = argparse.ArgumentParser(
            description=(
                "Check state of vicinity, routing table and fast-forwarding of nodes."
            )
        )
        parser.add_argument(
            "node",
            nargs="?",
            action=StoreNode,
            node_view=self.test.topology.nodes,
        )
        return parser

    @with_argparser("_check_parser")
    def do_check(self, args: argparse.Namespace) -> None:  # noqa: PLR0915, PLR0912
        node = args.node
        nodes = self.test.topology.nodes if node is None else iter([node])
        fail = False

        # print theoretical topology to aide in judgment of potential breakage
        if not self.quiet:
            buffer = self.imager.image_topology(dpi=100)
            img = Image.open(buffer)
            self.current_image = AutoImage(img)
            self.current_image.set_size(height=20)
            self.current_image.draw(h_align="left", v_align="top", pad_height=1)

        print("Node: v e p r a")
        for node in nodes:
            # TODO: Check for evidence in nftables that all paths are setup
            # TODO: Check if all discovered neighbors of a vicinity node match

            tid = self.test.topology.tid(node)
            topo_node = self.test.topology.topology_nodes[tid]
            unknown_vicinity = topo_node.unknown_vicinity(node)
            unknown_vicinity = next(unknown_vicinity, None) is not None
            if unknown_vicinity:
                fail = True

            unknown_edges = topo_node.unknown_vicinity_edges(node)
            unknown_edges = next(unknown_edges, None) is not None
            if unknown_edges:
                fail = True

            missing_pathsetups = node.missing_vicinity_path_setups()
            if missing_pathsetups is None:
                print(f"{node:<3} ERR determining vicinity paths setup.")
                self._cmd_failed()
                return
            missing_pathsetups = next(missing_pathsetups, None) is not None
            if missing_pathsetups is None:
                fail = True

            missing_pathsetups_rt = node.missing_pathsetups_rt()
            if missing_pathsetups_rt is None:
                print(f"{node:<3} ERR determining routing table paths setup.")
                self._cmd_failed()
                return
            missing_pathsetups_rt = next(missing_pathsetups_rt, None) is not None
            if missing_pathsetups_rt is None:
                fail = True

            additional_vicinity = topo_node.additional_vicinity(node)
            try:
                next(additional_vicinity)
                additional_vicinity = True
                fail = True
            except StopIteration:
                additional_vicinity = False

            def _bool_char(b: bool) -> str:
                return "✗" if b else "✔"  # failure is truthy

            print(
                f"{node:<3} : "
                f"{_bool_char(unknown_vicinity)} "
                f"{_bool_char(unknown_edges)} "
                f"{_bool_char(missing_pathsetups)} "
                f"{_bool_char(missing_pathsetups_rt)} "
                f"{_bool_char(additional_vicinity)}",
            )

        if not self.quiet:
            print()
            print("v = Vicinity-Nodes in Vicinty Graph")
            print("e = Edges inside the vicinity radius")
            print("p = Paths inside the vicinity radius setup (nftables, known)")
            print("r = Paths of Contacts inside the Routing Table setup (known)")
            print("a = Additional nodes in Vicinity Graph")

        if fail:
            if not self.quiet:
                print()
                print("To further investigate failures type: VICINITY <nid>")
            self._cmd_failed()

    @property
    def _closest_to(self) -> argparse.ArgumentParser:
        parser = argparse.ArgumentParser(
            description=("Get the closest node to a key (SHA1).")
        )
        parser.add_argument(
            "key",
            type=str,
        )
        return parser

    @with_argparser("_closest_to")
    def do_closest_to(self, args: argparse.Namespace) -> None:
        hash_key = hashlib.sha256(args.key.encode("utf-8")).digest()[: NodeID.LENGTH]
        key_int = int.from_bytes(hash_key, byteorder="big")

        print(f"SHA-256 Hash: {key_int:0{NodeID.LENGTH}x}")
        print(f"    {key_int:0{NodeID.LENGTH * 8}b}")
        print()

        distances = {
            node: key_int ^ int.from_bytes(node.node_id, byteorder="big")
            for node in self.test.topology.nodes
        }
        distances = sorted(distances.items(), key=lambda i: i[1])

        print("XOR-Distances:")
        for n, d in distances:
            print(f"{n:>3} {d:0{NodeID.LENGTH * 8}b}")

    @property
    def _netns_parser(self) -> argparse.ArgumentParser:
        parser = argparse.ArgumentParser(
            description=("Get the network namespace name of a node.")
        )
        parser.add_argument(
            "node",
            action=StoreNode,
            node_view=self.test.topology.nodes,
        )
        return parser

    @with_argparser("_netns_parser")
    def do_netns(self, args: argparse.Namespace) -> None:
        node = args.node
        netns_name = node.id
        print(f"netnsname: {netns_name}")

    @property
    def _sleep_parser(self) -> argparse.ArgumentParser:
        parser = argparse.ArgumentParser(description=("Sleep for an amount of time."))
        parser.add_argument("seconds", type=int, help="amount to sleep in seconds")
        return parser

    @with_argparser("_sleep_parser")
    def do_sleep(self, args: argparse.Namespace) -> None:
        t_secs = args.seconds
        if t_secs <= 0:
            print("ERR: sleep time must be greater than zero")
            self._cmd_failed()
            return

        if not self.quiet:
            print(f"Sleeping for {t_secs} seconds...")
        time.sleep(t_secs)

    def do_exit(self, arg: str) -> bool:
        "Exit the debug shell"
        _ = arg

        print("Exiting...")
        # Kill all processes in the network namespaces
        # to make sure everything is cleaned up

        for node in self.test.topology.nodes:
            netns_name = node.id
            p = Popen(["ip", "netns", "pids", netns_name], stdout=PIPE)
            stdout, _ = p.communicate()
            if p.returncode == 0 and stdout:
                pids = stdout.decode().strip().split()
                if pids:
                    kill_cmd = f"kill {' '.join(pids)}"
                    _ = Popen(
                        f"ip netns exec {netns_name} {kill_cmd}", shell=True
                    ).communicate()

            if self.post_process_log_files:
                self.process_log_file(node.logfile)

        # TODO: still partially broken, python processes are not killed for some reason

        self.close()
        return True

    def onecmd(self, line: str) -> bool:
        """Intercept command execution to catch and display unhandled exceptions."""
        try:
            return super().onecmd(line)
        except Exception as e:
            print(f"Error: {e}")
            self._cmd_failed()
            return False  # Return False to keep the loop running

    def cmdloop(self, intro: Any | None = None) -> None:
        try:
            super().cmdloop(intro)
        except KeyboardInterrupt:
            self.do_exit("")

    # ----- record and playback -----

    def do_record(self, arg: str) -> None:
        "Save future commands to filename:  RECORD <file_name.cmd>"
        self.file = open(arg, "w")  # noqa: SIM115

    def do_playback(self, arg: str) -> None:
        "Playback commands from a file:  PLAYBACK <file_name.cmd>"
        self.close()
        with open(arg) as f:
            self.cmdqueue.extend(f.read().splitlines())

    def precmd(self, line: str) -> str:
        line = line.lower()
        if self.file and "playback" not in line:
            print(line, file=self.file)
        if self.print_cmd:
            print(line)
        self.current_image = None
        return line

    def close(self) -> None:
        if self.file:
            self.file.close()
            self.file = None
