import argparse
import os
from cmd import Cmd
import sys
import re
from subprocess import Popen, PIPE
from typing import Optional, Iterator
import json
import base64
import threading
import signal

import networkx as nx

import nest
from nest.topology import Node, connect, Address


from common import NodeConfig


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
        return Popen(f"ip netns exec {self.id} {cmd}", shell=True,
                     env=env_vars, stdout=logfile, stderr=logfile)

    def api_call(self, path: str, payload: str = None) -> Optional[str]:
        cmd = f"curl localhost:{self._api_port}/{path}"
        if payload:
            cmd += f" -d {payload}"

        p = self.exec(cmd, logfile=PIPE)
        stdout, _ = p.communicate()
        return stdout.decode("utf-8") if p.returncode == 0 else None

    def store(self, key: str, data: str) -> str:
        path = f"dht/store?key={key}"
        return self.api_call(path, data)

    def fetch(self, key: str) -> list[str]:
        path = f"dht/fetch?key={key}"
        res = self.api_call(path)
        if res is None:
            return list()
        else:
            # decode
            res = json.loads(res)
            return [base64.b64decode(value).decode("utf-8") for value in res]

    def routing_table(self) -> str:
        path = "_dev/routing-table"
        return self.api_call(path)

    def pn_table(self) -> str:
        path = "_dev/pn-table"
        return self.api_call(path)

    def vicinity_graph(self) -> str:
        path = "_dev/vicinity-graph"
        return self.api_call(path)

    def local_hashtable(self) -> str:
        path = "dht/_dev/local-hashtable"
        return self.api_call(path)

    def is_up(self) -> bool:
        path = "node-id"
        res = self.api_call(path)
        if res is None:
            return False
        res = json.loads(res)
        return "node-id" in res

    def __str__(self) -> str:
        return self.name


class NestTest:

    config: nx.Graph

    """
    Nest Test
    ===========

    This is a test for the Nest framework. It creates a topology and runs a connectivity test between nodes.

    Parameters
    ----------
    config : dict
        The configuration for the test.
    """

    def __init__(self, config: nx.Graph):
        self.topology = config

        nest.logging.info("Setting up the topology ...")

        # Create the Nest topology according to the configuration
        for tid in self.topology:
            node = KIRANode(f'n{tid}')
            node.enable_ip_forwarding(True, True)
            self.topology.nodes[tid]["node"] = node

        nest.logging.info("Setting up interfaces ...")
        for x, y in self.topology.edges:
            nx = self.topology.nodes[x]["node"]
            ny = self.topology.nodes[y]["node"]

            if_x, if_y = connect(nx, ny, f"n{x}n{y}", f"n{y}n{x}")
            if_x.set_address(self.topology.nodes[x]["config"].ipv6)
            if_y.set_address(self.topology.nodes[y]["config"].ipv6)

            # safe interfaces for later
            self.topology.edges[x, y][x] = if_x
            self.topology.edges[x, y][y] = if_y

        nest.logging.info("Starting daemons ...")
        for node, config in self.nodes():
            node_id = config.node_id

            logfile = f"n{node}.log"
            env_vars = os.environ.copy()
            env_vars["RUST_LOG_STYLE"] = "never"
            env_vars["NO_COLOR"] = "1"
            env_vars["RUST_LOG"] = "info"
            env_vars["RUST_BACKTRACE"] = "1"
            with open(logfile, 'w') as f:
                node.exec(f"./target/debug/kirad --root-id {node_id} --nftables-conf ./kirad/conf/nftables.conf",
                          logfile=f,
                          env_vars=env_vars)

    def nodes(self) -> Iterator[tuple[KIRANode, NodeConfig]]:
        return ((ndata["node"], ndata["config"]) for _tid, ndata in self.topology.nodes(data=True))


class DebugShell(Cmd):
    intro = "Welcome to the debug shell of nesttest.  Type help or ? to list commands.\n"
    prompt = '(debug)'
    file = None

    test: NestTest

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

            yield nid, str(tid)
            yield short_nid, str(tid)
            yield ipv6, str(tid)

    def _compile_re(self):
        self._replacement_map = dict(self._construct_replacement_map())
        print(self._replacement_map)

        replace_re = "|".join(re.escape(nid)
                              for nid in self._replacement_map.keys())
        ignore_case = f"(?i:{replace_re})"
        self._replace_re = re.compile(ignore_case)

    def sub_nid_tid(self, string: str) -> str:
        """
        Substitute Node-IDs with the corresponding ID used in the topology.

        Shortened Node-IDs of length 8
        and the IPv6-addresses of the nodes are also replaced.
        """

        def replace(match: re.Match):
            matched = match.group(0).lower()
            replace_with = self._replacement_map.get(matched, matched)
            return replace_with

        return self._replace_re.sub(replace, string)

    def _extract_tid(self, arg: str) -> (KIRANode, Optional[str]):
        args = arg.split(maxsplit=1)
        nid = args[0]
        scmd = args[1] if len(args) >= 2 else None

        # strip beginning n if present
        tid = re.match(r"n?(.+)", nid).group(1)
        node = self.test.topology.nodes[tid]["node"]
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
                    print(f'Pinging {x} -> {y} ...', end='\r')
                    ip_y = y_config.ipv6
                    ip_y = Address(ip_y)
                    result = x.ping(ip_y, packets=1, verbose=verbose)

                    if not verbose:
                        if result:
                            # overwrite line if failed
                            end = '\r' if failed else '\n'
                            print(f"Pinging {x} -> {y} ✓  ",
                                  end=end, flush=True)
                            continue
                        else:
                            print(
                                f"Pinging {x} -> {y} ✗          ", flush=True)

    def do_exec(self, arg):
        "Execute arbitrary command in the network namespace of node: EXECUTE <nid> <cmd>"
        node, cmd = self._extract_tid(arg)
        p = node.exec(cmd, logfile=sys.stdout)
        p.wait()
        print()

    def do_api(self, arg):
        "Issue arbitrary API call to node: API <nid> <rest_path>"
        node, path = self._extract_tid(arg)
        res = node.api_call(path)
        res = self.sub_nid_tid(res)
        print(res)

    def do_store(self, arg) -> str:
        "Store a key-value pair in the DHT: STORE <nid> <key> <value>"
        node, key_data = self._extract_tid(arg)
        key, data = key_data.split(maxsplit=1)
        print(node.store(key, data))

    def do_fetch(self, arg) -> str:
        "Obtain value of a key in the DHT: FETCH <nid> <key>"
        node, key = self._extract_tid(arg)
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

    def do_routing_table(self, arg) -> str:
        "Dumps routing table of node: ROUTING_TABLE <nid>"
        node, _ = self._extract_tid(arg)
        res = node.routing_table()
        res = self.sub_nid_tid(res)
        print(res)

    def do_pn_table(self, arg) -> str:
        "Dump physical neighbor table of node: PN_TABLE <nid>"
        node, _ = self._extract_tid(arg)
        res = node.pn_table()
        res = self.sub_nid_tid(res)
        print(res)

    def do_vicinity_graph(self, arg) -> str:
        "Dump vicinity graph of node: VICINITY_GRAPH <nid>"
        node, _ = self._extract_tid(arg)
        res = node.vicinity_graph()
        res = self.sub_nid_tid(res)
        print(res)

    def do_local_hashtable(self, arg) -> str:
        "Dump local hashtable of node: LOCAL_HASHTABLE <nid>"
        node, _ = self._extract_tid(arg)
        res = node.local_hashtable()
        print(res)

    def do_checkup(self, arg) -> str:
        "Check if nodes are up: CHECKUP [nid]"
        # check all of no node is specified
        if arg == "":
            down_nodes = [n for n, _ in self.test.nodes() if not n.is_up()]
            if len(down_nodes) == 0:
                print("All nodes are up!")
            else:
                for n in down_nodes:
                    print(f"Node {n} is down.")

            return
        node, _ = self._extract_tid(arg)
        is_up = node.is_up()
        if is_up:
            print(f"Node {node} is up")
        else:
            print(f"Node {node} is not up")

    def do_exit(self, arg):
        'Exit the debug shell'
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
        'Save future commands to filename:  RECORD rose.cmd'
        self.file = open(arg, 'w')

    def do_playback(self, arg):
        'Playback commands from a file:  PLAYBACK rose.cmd'
        self.close()
        with open(arg) as f:
            self.cmdqueue.extend(f.read().splitlines())

    def precmd(self, line):
        line = line.lower()
        if self.file and 'playback' not in line:
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
    parser = argparse.ArgumentParser(description='Nest Test Script')
    parser.add_argument('test_gml', type=str, help="The gml file")
    args = parser.parse_args()
    main(args)
