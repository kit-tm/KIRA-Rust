import argparse
import os
from cmd import Cmd
import sys
import re
from subprocess import Popen

import networkx as nx

import nest
from nest.topology import Node, connect, Address


from common import NodeConfig


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
            n = Node(f'n{idx}')
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
                node_exec(node,
                          f"./target/debug/kirad --root-id {
                              node_id} --nftables-conf ./kirad/conf/nftables.conf",
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

    def do_pingall(self, arg):
        'Ping all nodes'
        self.test.pingall()

    def do_exec(self, arg):
        nid, cmd = arg.split(maxsplit=1)
        idx = re.match(r"n?(\d+)", nid).group(1)
        idx = int(idx)
        node = self.test.nodes[idx]
        p = node_exec(node, cmd, logfile=sys.stdout)
        p.wait()
        print()

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


def node_exec(node: Node, cmd, env_vars=None, logfile=None):
    """
    Execute a command in the node's namespace.
    """
    if env_vars is None:
        env_vars = os.environ.copy()
    if logfile is None:
        print("No logfile provided, using stdout")
    return Popen(f"ip netns exec {node.id} {cmd}", shell=True,
                 env=env_vars, stdout=logfile, stderr=logfile)


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
