#! /bin/env python3
from mininet.net import Containernet
from mininet.cli import CLI
from mininet.link import Intf
from mininet.log import info, warn, setLogLevel

from ipaddress import IPv4Network

from subprocess import Popen, PIPE

setLogLevel('info')

BASE_KIRA_IMAGE = "kira-dns-tunnel:latest"

DCMD = "/usr/bin/supervisord -c /etc/supervisord.conf"
SYSCTLS = {
    'net.ipv6.conf.default.disable_ipv6': 0,
    'net.ipv6.conf.all.forwarding': 1,
}

INTERNAL_V4_PREFIX = IPv4Network("10.0.0.0/24")

def main():
    net = Containernet()
    try:
        setup(net, args.external_interface)

        info('*** Starting network\n')
        net.start()

        info('*** Running CLI\n')
        CLI(net)
    finally:
        info('*** Stopping network')
        net.stop()

def addDocker(net, name, image, internal_ipv4, env={}):
    env2 = dict({'KIRA_IPV4': internal_ipv4, 'RUST_LOG': "debug"})
    env2.update(env)
    return net.addDocker(name, dimage=image, sysctls=SYSCTLS,
                         ip=None, network_mode="none", dns=["127.0.0.1"],
                         dcmd=DCMD, environment=env2)

def setup(net, external_interface: Optional[str] = None):

    info('*** Adding docker containers\n')

    # nodes configured with 4in6 tunnels
    server = addDocker(net, 's1', SERVER_KIRA_IMAGE, "10.0.0.1", {"V4_TUNNEL_HOSTS": "c1.kira.internal:c2.kira.internal"})
    client1 = addDocker(net, 'c1', CLIENT_KIRA_IMAGE, "10.0.0.2", {"V4_TUNNEL_HOSTS": "s1.kira.internal"})
    client2 = addDocker(net, 'c2', CLIENT_KIRA_IMAGE, "10.0.0.3", {"V4_TUNNEL_HOSTS": "s1.kira.internal"})

    # kira
    d4, d5, d6, d7 = [addDocker(net, f"d{i}", BASE_KIRA_IMAGE, f"10.0.0.{i}") for i in range(4, 8)]

    # add links
    net.addLink(d1, d6)
    net.addLink(d2, d4)
    net.addLink(d3, d5)
    net.addLink(d4, d7)
    net.addLink(d4, d5)
    net.addLink(d5, d6)
    net.addLink(d7, d6)

def exec_node(node, cmd):
    info(f"Executing CMD on {node.name}: {cmd}")
    return node.popen(cmd, shell=True, stdout=PIPE).stdout.read(-1).decode()

if __name__ == "__main__":
    main()
