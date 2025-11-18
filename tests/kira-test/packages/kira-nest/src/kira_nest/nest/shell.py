import argparse
import re
import sys
import time
from cmd import Cmd
from collections.abc import Iterable, Iterator
from ipaddress import AddressValueError
from subprocess import PIPE, Popen
from typing import Any

import networkx as nx
from kira_common import NodeConfig
from nest.topology.address import Address
from PIL import Image
from term_image.image import AutoImage

from kira_nest.domain import KiraIP, NodeID, NodeIP, PathIP
from kira_nest.domain.forwarding import (
    FwdEntry,
    NodeIDEncapEntry,
    NodeIDFwdEntry,
    PathIDFwdEntry,
    PathIDSwapEntry,
)
from kira_nest.nest.imager import KIRAImager
from kira_nest.nest.link import KIRALink

from .node import KIRANode
from .test import KIRATest


# TODO: rewrite do_foo(arg) command argument parsing
#       using robust parsing provided by argparse
class DebugShell[T](Cmd):
    intro = (
        "Welcome to the debug shell of nesttest."
        "Type help or ? to list commands, exit to quit.\n"
    )
    prompt = "ntest> "
    file = None
    failure = False  # a command failed
    exit_on_failure = False  # set -e
    print_cmd = False  # set -x
    quiet = False

    def __init__(self, test: KIRATest[T]) -> None:
        super().__init__()
        self.test = test
        self.imager = KIRAImager(test)

        self._compile_re()

    def _construct_replacement_map(self) -> Iterator[tuple[str, str]]:
        for tid, node_cfg in self.test.topology.configs():
            nid = node_cfg.node_id
            ipv6 = node_cfg.ipv6
            short_nid = nid[:8]
            replacement = f"${tid}$"

            yield nid, f"{replacement:<{len(nid)}}"
            yield short_nid, f"{replacement:<{len(short_nid)}}"
            yield ipv6, f"{replacement:<{len(ipv6)}}"

    def _compile_re(self) -> None:
        self._replacement_map = dict(self._construct_replacement_map())
        replace_re = "|".join(re.escape(nid) for nid in self._replacement_map)
        ignore_case = f"(?i:{replace_re})"
        self._replace_re = re.compile(ignore_case)

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
        scmd = args[1] if len(args) == 2 else None  # noqa: PLR2004

        node = self.test.topology.nodes.by_name(node_name)
        if node is None:
            return None, arg
        return (node, scmd)

    @classmethod
    def _parse_kira_ip(cls, ip: str | None) -> KiraIP | None:
        try:
            return NodeIP(ip)
        except AddressValueError:
            print(f"ERR: {ip} is not a valid IPv6-address")
        except ValueError:
            try:
                return PathIP(ip)
            except AddressValueError:
                print(f"ERR: {ip} is not a valid IPv6-address")
            except ValueError:
                print(f"ERR: {ip} has to be a Node- or Path-IP.")

    def do_pingall(self, arg: str) -> None:
        "Ping all nodes: PINGALL [-f,--failed] [-v,--verbose]"

        # process flags
        args = arg.split()
        failed = "-f" in args or "--failed" in args or self.quiet

        verbose = 2 if "-v" in args or "--verbose" in args else 0

        for x in self.test.topology.nodes:
            for y in self.test.topology.nodes:
                if x != y:
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

    def do_exec(self, arg: str) -> None:
        "Execute arbitrary command in the network namespace of node: EXEC <nid> <cmd>"
        node, cmd = self._extract_node(arg)
        if node is None:
            print(
                f"ERR: Node '{cmd}' not found.\n"
                "To get a list of available nodes type NODES."
            )
            self._cmd_failed()
            return
        if cmd is None:
            print("Provide a command to execute: EXECUTE <nid> <cmd>")
            self._cmd_failed()
            return

        p = node.exec(cmd, logfile=sys.stdout)
        exit_code = p.wait()
        if exit_code != 0:
            self._cmd_failed()
        print()

    def do_api(self, arg: str) -> None:
        "Issue arbitrary API call to node: API <nid> <rest_path>"
        node, path = self._extract_node(arg)
        if node is None:
            print(
                f"ERR: Node '{path}' not found.\n"
                "To get a list of available nodes type NODES."
            )
            self._cmd_failed()
            return
        if path is None:
            print("Provide an API-Path: API <nid> <rest_path>")
            self._cmd_failed()
            return

        res = node.api.call(path)
        if res is None:
            print("ERR: API call failed")
            self._cmd_failed()
            return
        res = self.sub_nid_name(res)
        print(res)

    def do_node_id(self, arg: str) -> None:
        "Obtain Node-Id: NODE_ID <nid>"
        node, _arg = self._extract_node(arg)
        if node is None:
            print(
                f"ERR: Node '{_arg}' not found.\n"
                "To get a list of available nodes type NODES."
            )
            self._cmd_failed()
            return

        node_id = node.api.node_id()
        if node_id is None:
            print("ERR: Unable to reach kira API backend of node.")
        else:
            print(node_id)

    def do_store(self, arg: str) -> None:
        "Store a key-value pair in the DHT: STORE <nid> <key> <value>"
        node, key_data = self._extract_node(arg)
        if node is None:
            print(
                f"ERR: Node '{key_data}' not found.\n"
                "To get a list of available nodes type NODES."
            )
            self._cmd_failed()
            return
        if key_data is None:
            print("Provide a key and value: STORE <nid> <key> <value>")
            self._cmd_failed()
            return

        key_data = key_data.split(maxsplit=1)
        if len(key_data) != 2:  # noqa: PLR2004
            print("Provide a key and value: STORE <nid> <key> <value>")
            self._cmd_failed()
            return
        key, data = key_data
        # TODO: parse result of store to catch failure
        print(node.api.store(key, data))

    def do_fetch(self, arg: str) -> None:
        "Obtain value of a key in the DHT: FETCH <nid> <key>"
        node, key = self._extract_node(arg)
        if node is None:
            print(
                f"ERR: Node '{key}' not found.\n"
                "To get a list of available nodes type NODES."
            )
            self._cmd_failed()
            return
        if key is None:
            print("Provide a key to fetch: FETCH <nid> <key>")
            self._cmd_failed()
            return

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

    def do_routing_table(self, arg: str) -> None:
        "Dumps routing table of node: ROUTING_TABLE <nid>"
        node, _arg = self._extract_node(arg)
        if node is None:
            print(
                f"ERR: Node '{_arg}' not found.\n"
                "To get a list of available nodes type NODES."
            )
            self._cmd_failed()
            return

        res = node.api.routing_table()
        if res is None:
            print("ERR: API call failed")
            self._cmd_failed()
            return
        res = self.sub_nid_name(res)
        print(res)

    def do_uln_table(self, arg: str) -> None:
        "Dump physical neighbor table of node: ULN_TABLE <nid>"
        node, _arg = self._extract_node(arg)
        if node is None:
            print(
                f"ERR: Node '{_arg}' not found.\n"
                "To get a list of available nodes type NODES."
            )
            self._cmd_failed()
            return

        res = node.api.uln_table()
        if res is None:
            print("ERR: API call failed")
            self._cmd_failed()
            return
        res = self.sub_nid_name(res)
        print(res)

    def do_vicinity_graph(self, arg: str) -> None:
        "Dump vicinity graph of node: VICINITY_GRAPH <nid>"
        node, _arg = self._extract_node(arg)
        if node is None:
            print(
                f"ERR: Node '{_arg}' not found.\n"
                "To get a list of available nodes type NODES."
            )
            self._cmd_failed()
            return

        res = node.api.vicinity_graph()
        if res is None:
            print("ERR: API call failed")
            self._cmd_failed()
            return
        res = self.sub_nid_name(res)
        print(res)

    def do_local_hashtable(self, arg: str) -> None:
        "Dump local hashtable of node: LOCAL_HASHTABLE <nid>"
        node, _arg = self._extract_node(arg)
        if node is None:
            print(
                f"ERR: Node '{_arg}' not found.\n"
                "To get a list of available nodes type NODES."
            )
            self._cmd_failed()
            return

        res = node.api.local_hashtable()
        if res is None:
            print("ERR: API call failed")
            self._cmd_failed()
            return
        print(res)

    def do_checkup(self, arg: str) -> None:
        "Check if nodes are up: CHECKUP [nid]"
        # check all if no node is specified
        if arg == "":
            all_up = True
            for n in self.test.topology.nodes:
                if n.is_down():
                    print(f"Node {n} is down.")
                    all_up = False
                    self._cmd_failed()
            if all_up:
                print("All nodes are up!")

            return

        node, _arg = self._extract_node(arg)
        if node is None:
            print(
                f"ERR: Node '{_arg}' not found.\n"
                "To get a list of available nodes type NODES."
            )
            self._cmd_failed()
            return

        if node.is_up():
            print(f"Node {node} is up")
        else:
            print(f"Node {node} is not up")
            self._cmd_failed()

    def do_next_ip(self, arg: str) -> None:
        node, ip = self._extract_node(arg)
        if node is None:
            print(
                f"ERR: Node '{ip}' not found.\n"
                "To get a list of available nodes type NODES."
            )
            self._cmd_failed()
            return
        kira_ip = self._parse_kira_ip(ip)
        if kira_ip is None:
            return

        next_ip = node.next_ip(kira_ip)
        print()
        print(next_ip)
        if next_ip is None:
            self._cmd_failed()

    def do_next_hop(self, arg: str) -> None:
        node, ip = self._extract_node(arg)
        if node is None:
            print(
                f"ERR: Node '{ip}' not found.\n"
                "To get a list of available nodes type NODES."
            )
            self._cmd_failed()
            return
        kira_ip = self._parse_kira_ip(ip)
        if kira_ip is None:
            return

        next_hop = node.next_hop(kira_ip)
        print()
        print(next_hop)
        if next_hop is None:
            self._cmd_failed()

    def do_path(self, arg: str) -> None:
        "Lookup Path-ID on the node: PATH <nid> [path-ip]"
        node, ip = self._extract_node(arg)
        if node is None:
            print(
                f"ERR: Node '{ip}' not found.\n"
                "To get a list of available nodes type NODES."
            )
            self._cmd_failed()
            return

        if ip is None:
            paths = node.paths_routing_table()
            if paths is None:
                self._cmd_failed()
                return
        else:
            ip = self._parse_kira_ip(ip)
            if ip is None:
                self._cmd_failed()
                return
            if ip is not isinstance(ip, PathIP):
                # TODO: display error message
                return
            assert isinstance(ip, PathIP)

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

    def do_traceroute(self, arg: str) -> None:
        "Traceroute (all) forwarding path: TRACEROUTE [<nid_x> <nid_y>]"

        # trace all paths
        if arg == "":
            for x in self.test.topology.nodes:
                for y in self.test.topology.nodes:
                    success = self.test.traceroute(x, y, verbose=False)
                    # only show unsuccessful traceroutes
                    if not success:
                        self._cmd_failed()
                        self.test.traceroute(x, y, verbose=True)
                        print()
                        print("==============================================")
                        print()

            return

        x, argv = self._extract_node(arg)
        if x is None:
            print(
                f"ERR: Node '{x}' not found.\n"
                "To get a list of available nodes type NODES."
            )
            self._cmd_failed()
            return
        if argv is None:
            print("Destination not found. Usage: TRACEROUTE <nid_x> <nid_y>")
            self._cmd_failed()
            return

        y, argv = self._extract_node(argv)
        if y is None:
            print(
                f"ERR: Node '{argv}' not found.\n"
                "To get a list of available nodes type NODES."
            )
            self._cmd_failed()
            return

        traceable = self.test.traceroute(x, y, verbose=not self.quiet)
        if not traceable:
            self._cmd_failed()

    def do_link(self, arg: str) -> None:
        "Sets link (or all links of node) up or down: LINK <DOWN/UP> <nid_x> [nid_y]"

        # parse args
        match arg.split(maxsplit=1):
            case mode, argv:
                pass
            case _:
                print("ERR: Unexpected command syntax. LINK <DOWN/UP> <nid_x> [nid_y]")
                return
        x, argv = self._extract_node(argv)
        if x is None:
            print(
                f"ERR: Node '{argv}' not found.\n"
                "To get a list of available nodes type NODES."
            )
            self._cmd_failed()
            return

        x_tid = self.test.topology.tid(x)
        assert x_tid is not None  # because we just parsed the name to get x
        if argv is not None:
            y, argv = self._extract_node(argv)
            if y is None:
                print(
                    f"ERR: Node '{argv}' not found.\n"
                    "To get a list of available nodes type NODES."
                )
                self._cmd_failed()
                return
            y_tid = self.test.topology.tid(y)
            assert y_tid is not None
            ys_tid = [y_tid]
        else:
            # all links from x
            ys_tid = [y_tid for y_tid, _ in self.test.topology.links[x_tid, ...]]

        mode = mode.lower()
        match mode:
            case "up":
                mode_sym = "✔"
                mode_fn = KIRALink.up
            case "down":
                mode_sym = "✗"
                mode_fn = KIRALink.down
            case _:
                print(f"ERR: Unknown mode {mode}")
                self._cmd_failed()
                return

        for y_tid in ys_tid:
            y = self.test.topology.nodes[y_tid]
            link = self.test.topology.links[x_tid, y_tid]
            assert isinstance(link, KIRALink)
            mode_fn(link)

            if not self.quiet:
                print(f"{x:>3} -{mode_sym}- {y:>3} ...")

    def do_down(self, arg: str) -> None:
        "Alias for LINK DOWN <...>"
        self.do_link(f"DOWN {arg}")

    def do_up(self, arg: str) -> None:
        "Alias for LINK UP <...>"
        self.do_link(f"UP {arg}")

    def do_links(self, arg: str) -> None:
        "List links in topology: LINKS [nid]"

        if arg == "":
            for x_tid, y_tid, link in self.test.topology.links:
                x = self.test.topology.nodes[x_tid]
                y = self.test.topology.nodes[y_tid]
                up_indicator = "-" if link.is_up() else "✗"
                print(f"{x:>3} -{up_indicator}- {y:>3}")
        else:
            x, _arg = self._extract_node(arg)
            if x is None:
                print(
                    f"ERR: Node '{_arg}' not found.\n"
                    "To get a list of available nodes type NODES."
                )
                self._cmd_failed()
                return
            x_tid = self.test.topology.tid(x)
            for y_tid, link in self.test.topology.links[x_tid, ...]:
                y = self.test.topology.nodes[y_tid]
                up_indicator = "-" if link.is_up() else "✗"
                print(f"{x:>3} -{up_indicator}- {y:>3}")

    def do_edges(self, arg: str) -> None:
        "Alias for LINKS: EDGES [nid]"
        self.do_links(arg)

    def do_nodes(self, arg: str) -> None:
        "List all nodes present in the topology: NODES"
        _ = arg
        for n in self.test.topology.nodes:
            nid = n.node_id
            print(f"{n:>3} {nid}")

    def do_vicinity(self, arg: str) -> None:  # noqa: PLR0915, PLR0912
        "Get vicinity of <nid>: VICINITY <nid>"
        node, _arg = self._extract_node(arg)
        if node is None:
            print(
                f"ERR: Node '{_arg}' not found.\n"
                "To get a list of available nodes type NODES."
            )
            self._cmd_failed()
            return
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

    def do_check(self, arg: str) -> None:  # noqa: PLR0915, PLR0912
        """
        Check state of vicinity, routing table and fast-forwarding of nodes: CHECK [nid]
        """

        if arg == "":
            nodes = self.test.topology.nodes
        else:
            node, _arg = self._extract_node(arg)
            if node is None:
                print(
                    f"ERR: Node '{_arg}' not found.\n"
                    "To get a list of available nodes type NODES."
                )
                return
            nodes = iter([node])

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

    def do_netns(self, arg: str) -> None:
        "Get network namespace name of node <nid>: NETNS [nid]"
        node, _arg = self._extract_node(arg)
        if node is None:
            print(
                f"ERR: Node '{_arg}' not found.\n"
                "To get a list of available nodes type NODES."
            )
            self._cmd_failed()
            return

        netns_name = node.id
        print(f"netnsname: {netns_name}")

    def do_sleep(self, arg: str) -> None:
        "Sleep for an amount of time: SLEEP <seconds>"
        if arg == "":
            print("Usage: SLEEP <seconds>")
            self._cmd_failed()
            return

        try:
            t_secs = float(arg)
        except ValueError:
            print(f"ERR: {arg} is not an valid time duration in seconds")
            self._cmd_failed()
            return
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
            p = Popen(f"ip netns pids {netns_name}", shell=True, stdout=PIPE)
            stdout, _ = p.communicate()
            if p.returncode == 0 and stdout:
                pids = stdout.decode().strip().split()
                if pids:
                    kill_cmd = f"kill {' '.join(pids)}"
                    _ = Popen(
                        f"ip netns exec {netns_name} {kill_cmd}", shell=True
                    ).communicate()

        # TODO: still partially broken, python processes are not killed for some reason

        self.close()
        return True

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

    def do_show_graph(self, arg: str) -> None:
        "Show the current graph of the topology: SHOW_GRAPH"
        _ = arg

        buffer = self.imager.image_topology(dpi=400)
        img = Image.open(buffer)
        self.current_image = AutoImage(img)
        self.current_image.draw()

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


def build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Nest Test Script")
    parser.add_argument("test_gml", type=str, help="The gml file")
    parser.add_argument(
        "--otel",
        action="store_true",
        help="Enable open telemetry exports on all nodes",
    )
    parser.add_argument(
        "-q",
        "--quiet",
        action="store_true",
        help="Makes commands less verbose",
    )
    parser.add_argument(
        "filename",
        nargs="?",
        help="Commands to execute non-interactively",
        type=argparse.FileType("r"),
    )
    return parser


def run_shell() -> None:
    parser = build_arg_parser()
    args = parser.parse_args()

    # Load the configuration from the GML file
    graph = nx.readwrite.read_gml(args.test_gml)

    for node in graph.nodes:
        cfg = NodeConfig(**graph.nodes[node]["config"])
        # enable otel for all nodes
        if args.otel:
            cfg.otel = True

        graph.nodes[node]["config"] = cfg

    # Create and run the test
    test = KIRATest[str](graph)
    shell = DebugShell(test)
    shell.quiet = args.quiet

    if args.filename is not None:
        # non-interactive
        shell.exit_on_failure = True
        shell.print_cmd = True

        for cmd_line in args.filename.read().splitlines():
            print(cmd_line)
            shell.onecmd(cmd_line)
            if shell.failure:
                print("Last command failed. Exiting...")
                break
            print()
        shell.do_exit("")
    else:
        # interactive
        shell.cmdloop()

    exit_code = 1 if shell.failure else 0
    sys.exit(exit_code)


if __name__ == "__main__":
    run_shell()
