#!/usr/bin/env python3

import argparse
import networkx as nx
import random
import time
from dataclasses import dataclass
import json

from mininet.net import Containernet
from mininet.cli import CLI
from mininet.log import info, warn, setLogLevel
setLogLevel("info")

from common import NodeConfig

DCMD = "/usr/bin/supervisord -c /etc/supervisor/supervisord.conf"
SYSCTLS = {
    'net.ipv6.conf.default.disable_ipv6': 0,
    'net.ipv6.conf.all.forwarding': 1,
}

class TestBase:
    pass

class ConnectivityTest(TestBase):
    def run(config, net): 
        passed = True
        for n1 in config.nodes:
            n1 = config.nodes[n1]
            for n2 in config.nodes:
                n2 = config.nodes[n2]
                print('\r\033[K' + f'ConnectivityTest: Pinging {n1["config"].name} -> {n2["config"].name} ...', end='')
                connected = ConnectivityTest.ping(n1, n2)
                if not connected:
                    passed = False
                    print("failed!")

        print("")
        return passed

    def ping(n1, n2):
        p = n1["container"].popen(f"ping -c 1 -W 1 {n2['config'].ipv6}".split())
        p.wait()
        return p.returncode == 0
        

class TestRunner:
    def __init__(self, config):
        self.topology = config

    def run(self):
        net = Containernet()
        try:
            self.create(net) 
            net.start()
         
            input("Press Enter to start the connectivity test")
            
            if ConnectivityTest.run(self.topology, net):
                info("ConnectivityTest passed!")
                CLI(net)
            else:
                warn("ConnectivityTest failed!")
                CLI(net)
            
            failing_links = list((x,y) for x, y, data in self.topology.edges.data() if "fail" in data)
            if len(failing_links) != 0:
                for x, y in failing_links:
                   info(f"Setting link {x}<->{y} down!")
                   net.configLinkStatus(f"k{x}",f"k{y}", "down")
 
                time.sleep(10)
 
                if ConnectivityTest.run(self.topology, net):
                    info("ConnectivityTest passed!")
                    CLI(net)
                else:
                    warn("ConnectivityTest failed!")
                    CLI(net)
                for x, y in failing_links:
                   info(f"Setting link {x}<->{y} up!")
                   net.configLinkStatus(f"k{x}",f"k{y}", "up")
 
                time.sleep(10)
 
                if ConnectivityTest.run(self.topology, net):
                    info("ConnectivityTest passed!")
                    CLI(net)
                else:
                    warn("ConnectivityTest failed!")
                    CLI(net)

        finally:
           input("Press Enter to stop the network")
           net.stop()

    def create(self, net):
        node_map = {}
        env_vars = {}

        for idx, node in enumerate(self.topology.nodes):
            node_id = self.topology.nodes[node]["config"].node_id
            ip6_address = self.generate_ipv6_address(node_id)
            env_var_name = f"n{idx}"
            env_vars[env_var_name] = ip6_address
            self.topology.nodes[node]["config"].env_var_name = env_var_name
            node_map[env_var_name] = node_id.upper()

        with open('idmap.json', 'w') as file:
            json.dump(node_map, file)

        for node in self.topology.nodes:
            config = self.topology.nodes[node]["config"]
            container = self.create_container(net, config, env_vars)
            self.topology.nodes[node]["container"] = container

        for (x, y) in self.topology.edges:
            x = self.topology.nodes[x]["container"]
            y = self.topology.nodes[y]["container"]
            net.addLink(x,y)

    def create_container(self, net, config, env_vars):
        container_env_vars = {**env_vars, 'NODE_ID': config.node_id, 'RUST_LOG': 'debug'}
        return net.addDocker(config.name, dimage=config.image, sysctls=SYSCTLS,
                             ip=None, network_mode="none", dns=["127.0.0.1"],
                             dcmd=DCMD, environment=container_env_vars)

    def generate_ipv6_address(self, node_id):
        hex_groups = [node_id[i:i+4].ljust(4, '0') for i in range(0, len(node_id), 4)]
        return 'fc00:' + ':'.join(hex_groups)

def main(args):
    G = nx.readwrite.read_gml(args.test_gml)
    
    for node in G.nodes:
        G.nodes[node]["config"] = NodeConfig(**G.nodes[node]["config"])

    runner = TestRunner(G)
    runner.run()

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description='Test Execution Script with additional features')
    parser.add_argument('test_gml', type=str, help="The gml file")
    args = parser.parse_args()
    main(args)
