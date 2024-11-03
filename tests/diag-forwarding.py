"""
This is an automated Path tracer for KIRA
just using docker (runs outside containernet)
Author(s): Roland Bless (2024)
"""

import docker
import re
import argparse
import networkx as nx
import random
import time
from dataclasses import dataclass
import ipaddress

from common import NodeConfig



def normalize_ipv6_address(ipv6_address):
    try:
        # Create an IPv6Address object
        ipv6_obj = ipaddress.IPv6Address(ipv6_address)
        # Return the normalized (compressed) address
        return str(ipv6_obj)
    except ipaddress.AddressValueError:
        return None

def get_fwtable_entry(container_name, pathid):
    client = docker.from_env()
    container = client.containers.get(container_name)
    # command:
    #nft get element ip6 kira forwardmap { fcaa:269b:4845:8a0e:60b9:1c97:c5e9:bfe1 }
    # output:
    #table ip6 kira {
    #    map forwardmap {
    #            type ipv6_addr : ipv6_addr
    #            elements = { fcaa:269b:4845:8a0e:60b9:1c97:c5e9:bfe1 : fcaa:9129:bb40:d706:c149:3e42:2d1c:48b }
    #    }
    #}
    cmd = 'nft get element ip6 kira forwardmap {' + pathid + ' }'
    result = container.exec_run(cmd)
    # Decode and parse the command output
    output = result.output.decode('utf-8')
    return parse_nft_output(output)

def parse_nft_output(output):
    errormsg = re.search(r'^(Error): Could not process rule: No such file or directory', output)
    if errormsg:
        return ""
    else:
        match = re.search(r'elements = { [a-fA-F0-9:]+ : ([a-fA-F0-9:]+) }',output)
        if match:
            return match.group(1)
        else:
            return ""

def get_ip_route(container_name, destaddr):
    client = docker.from_env()
    container = client.containers.get(container_name)

    # Execute the 'ip route get' command in the container
    cmd = 'ip route get ' + destaddr
    result = container.exec_run(cmd)
    
    # Decode and parse the command output
    output = result.output.decode('utf-8')
    #print(f"ran cmd on container {container_name} cmd: {cmd}")
    #print(f"result: {output}")
    return parse_ip_route(output)

def parse_ip_route(output):
    # Using regular expressions to parse the output
    matchpid = re.search(r'encap ip6 .* src :: dst ([a-fA-F0-9:]+)', output)
    if matchpid:
        pathid = matchpid.group(1)
        return {'pathid': pathid, 'nexthop': "", 'dev': ""}
    matchnexthop = re.search(r'via ([a-fA-F0-9:]+) dev ([\w\d]+)', output)
    if matchnexthop:
        nexthop = matchnexthop.group(1)
        dev = matchnexthop.group(2)
        return {'pathid': "", 'nexthop': nexthop, 'dev': dev}
    else:
        local = re.search(r'^local',output)
        if local:
            return {'pathid': "", 'nexthop': "local", 'dev': ""}

    return {'pathid': "", 'nexthop': "", 'dev': ""}


class TraceRouteInfo:
    def __init__(self, config):
        self.topology = config
        self.node_map_name = {} # name to config
        self.node_map_nodeid = {} # NodeIDv6 to name
        for node in self.topology.nodes:
            name = self.topology.nodes[node]["config"].name
            self.node_map_name[name] = self.topology.nodes[node]
            self.node_map_nodeid[normalize_ipv6_address(self.topology.nodes[node]["config"].ipv6)] = name

    def get_full_route(self,start_container_name,destaddr):
        finished = False
        current_container_name = start_container_name
        current_addr = destaddr
        while not finished:
            print(f"{current_container_name} to {current_addr}")
            #print(f"\nAsking {current_container_name} for route to {current_addr}")
            # for PathIDs we first check for a local mapping inPID -> outPID
            if re.search(r'^fcaa:',current_addr):
                result = get_fwtable_entry(current_container_name,current_addr)
                if result:
                    print(f"PathID: {current_addr} -> {result} ");
                    current_addr = result

            ip_route_info = get_ip_route(current_container_name, current_addr)
            if ip_route_info:
                if ip_route_info['pathid']:
                    print(f"-> PathID: {ip_route_info['pathid']} ");
                    current_addr = ip_route_info['pathid']
                if ip_route_info['nexthop']:
                    if ip_route_info['nexthop'] == 'local':
                        finished = True
                        print(f" NextHop: local")
                    else:
                        print(f" NextHop: {ip_route_info['nexthop']} ({self.node_map_nodeid[ip_route_info['nexthop']]})")
                        current_container_name = self.node_map_nodeid[ip_route_info['nexthop']]
                        if (not current_container_name):
                            printf(f"\nContainer not found for {ip_route_info['nexthop']}");
                            finished = true
                        current_container_name = "mn." + current_container_name
                #print(f"PathID: {ip_route_info['pathid']}, NextHop: {ip_route_info['nexthop']}, Interface: {ip_route_info['dev']}")
            else:
                print("Could not parse IP route information.")
                finished = True
        return current_addr

    def run(self, startnode, destnode):
        print(f"from {startnode} -> {destnode}")
        start_container_name = "mn."+startnode
        destaddr = self.node_map_name[destnode]["config"].ipv6
        finished = False
        lastaddr = self.get_full_route(start_container_name, destaddr)
        if self.node_map_nodeid[lastaddr] == destnode:
            print(f"Path complete. Arrived at {destnode}")

def main(args):
    G = nx.readwrite.read_gml(args.test_gml)
    
    for node in G.nodes:
        G.nodes[node]["config"] = NodeConfig(**G.nodes[node]["config"])

    runner = TraceRouteInfo(G)
    runner.run(args.startnode,args.destnode)

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description='Diagnose KIRA forwarding script')
    parser.add_argument('test_gml', type=str, help="The gml file")
    parser.add_argument('startnode', type=str, help="start node name")
    parser.add_argument('destnode', type=str, help="destination node name")
    args = parser.parse_args()
    main(args)
