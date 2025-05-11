import argparse
import os
from cmd import Cmd
import sys
import re
from subprocess import Popen, PIPE
from typing import Optional
import json
import base64

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
        return self.api_call(path) is not None


class NestTest:
    """
    Nest Test
    ===========

    This is a test for the Nest framework. It creates a topology and runs a connectivity test between nodes.

    Parameters
    ----------
    config : dict
        The configuration for the test.
    """

    def __init__(self, config):
        self.topology = config
        self.nodes = []

        nest.logging.info("Setting up the topology ...")

        # Create the Nest topology according to the configuration
        for idx, node in enumerate(self.topology.nodes):
            n = KIRANode(f'n{idx}')
            n.enable_ip_forwarding(True, True)
            self.nodes.append(n)

        nest.logging.info("Setting up interfaces ...")

        for x, y in self.topology.edges:
            if_x, if_y = connect(self.nodes[int(x)], (self.nodes[int(y)]), f"n{
                                 x}n{y}", f"n{y}n{x}")
            if_x.set_address(self.topology.nodes[str(x)]["config"].ipv6)
            if_y.set_address(self.topology.nodes[str(y)]["config"].ipv6)

        nest.logging.info("Starting daemons ...")
        for idx, node in enumerate(self.nodes):
            logfile = f"n{idx}.log"
            node_id = self.topology.nodes[str(idx)]["config"].node_id
            ipv6 = self.topology.nodes[str(idx)]["config"].ipv6
            env_vars = os.environ.copy()
            env_vars["RUST_LOG_STYLE"] = "never"
            env_vars["NO_COLOR"] = "1"
            env_vars["RUST_LOG"] = "info"
            env_vars["RUST_BACKTRACE"] = "1"
            with open(logfile, 'w') as f:
                node.exec(f"./target/debug/kirad --root-id {node_id} --nftables-conf ./kirad/conf/nftables.conf",
                          logfile=f,
                          env_vars=env_vars)

    def pingall(self):
        for x in self.nodes:
            for (idx_y, y) in enumerate(self.nodes):
                if x != y:
                    print(f'Pinging {x} -> {y} ...', end='')
                    result = x.ping(
                        Address(self.topology.nodes[str(idx_y)]["config"].ipv6), packets=1)
                    if result:
                        continue
                    else:
                        print("failed!")


class DebugShell(Cmd):
    intro = "Welcome to the debug shell of nesttest.  Type help or ? to list commands.\n"
    prompt = '(debug)'
    file = None

    test: NestTest

    def __init__(self, test: NestTest):
        super().__init__()
        self.test = test

    def _extract_nid(self, arg: str) -> (KIRANode, Optional[str]):
        args = arg.split(maxsplit=1)
        nid = args[0]
        scmd = args[1] if len(args) >= 2 else None

        idx = re.match(r"n?(\d+)", nid).group(1)
        idx = int(idx)
        node = self.test.nodes[idx]
        return (node, scmd)

    def do_pingall(self, arg):
        'Ping all nodes'
        self.test.pingall()

    def do_exec(self, arg):
        "Execute arbitrary command in the network namespace of node: EXECUTE <nid> <cmd>"
        node, cmd = self._extract_nid(arg)
        p = node.exec(cmd, logfile=sys.stdout)
        p.wait()
        print()

    def do_api(self, arg):
        "Issue arbitrary API call to node: API <nid> <rest_path>"
        node, path = self._extract_nid(arg)
        res = node.api_call(path)
        print(res)

    def do_store(self, arg) -> str:
        "Store a key-value pair in the DHT: STORE <nid> <key> <value>"
        node, key_data = self._extract_nid(arg)
        key, data = key_data.split(maxsplit=1)
        print(node.store(key, data))

    def do_fetch(self, arg) -> str:
        "Obtain value of a key in the DHT: FETCH <nid> <key>"
        node, key = self._extract_nid(arg)
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
        node, _ = self._extract_nid(arg)
        res = node.routing_table()
        print(res)

    def do_pn_table(self, arg) -> str:
        "Dump physical neighbor table of node: PN_TABLE <nid>"
        node, _ = self._extract_nid(arg)
        res = node.pn_table()
        print(res)

    def do_vicinity_graph(self, arg) -> str:
        "Dump vicinity graph of node: VICINITY_GRAPH <nid>"
        node, _ = self._extract_nid(arg)
        res = node.vicinity_graph()
        print(res)

    def do_local_hashtable(self, arg) -> str:
        "Dump local hashtable of node: LOCAL_HASHTABLE <nid>"
        node, _ = self._extract_nid(arg)
        res = node.local_hashtable()
        print(res)

    def do_checkup(self, arg) -> str:
        "Check if nodes are up: CHECKUP [nid]"
        # check all of no node is specified
        if arg == "":
            down_nodes = [n for n in self.test.nodes if not n.is_up()]
            if len(down_nodes) == 0:
                print("All nodes are up!")
            else:
                for n in down_nodes:
                    print(f"Node {n.name} is down.")

            return
        node, _ = self._extract_nid(arg)
        is_up = node.is_up()
        if is_up:
            print(f"Node {node.name} is up")
        else:
            print(f"Node {node.name} is not up")

    def do_exit(self, arg):
        'Exit the debug shell'
        self.close()
        return True

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
