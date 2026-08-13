#! /bin/env python3
from mininet.net import Containernet
from mininet.cli import CLI
from mininet.link import Intf
from mininet.log import info, warn, setLogLevel

from subprocess import Popen, PIPE

setLogLevel('info')

BASE_KIRA_IMAGE = "kira-example-base:latest"
CLIENT_KIRA_IMAGE = "kira-example-client:latest"
SERVER_KIRA_IMAGE = "kira-example-server:latest"

DCMD = "/usr/bin/supervisord -c /etc/supervisor/supervisord.conf"
SYSCTLS = {
    'net.ipv6.conf.default.disable_ipv6': 0,
    'net.ipv6.conf.all.forwarding': 1,
}

def main():
    net = Containernet()
    try:
        setup(net)

        info('*** Starting network\n')
        net.start()

        info('*** Running CLI\n')
        CLI(net)
    finally:
        info('*** Stopping network')
        net.stop()

# helper function to add a container
def addDocker(net, name, image, internal_ipv4, env={}):
    env2 = dict({'KIRA_IPV4': internal_ipv4, 'RUST_LOG': "debug"})
    env2.update(env)
    return net.addDocker(name, dimage=image, sysctls=SYSCTLS,
                         ip=None, network_mode="none", dns=["127.0.0.1"],
                         dcmd=DCMD, environment=env2)

def setup(net):

    info('*** Adding docker containers\n')

    # nodes configured with 4in6 tunnels:           tunnelipv4                       hostnames of remote endpoints         register as roll.kira.internal
    #                                                vvvvvvv                         vvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvv    vvvvvvvvvvvvvvvvvvv
    server = addDocker(net, 's1', SERVER_KIRA_IMAGE, "10.0.0.1", {"V4_TUNNEL_HOSTS": "c1.kira.internal:c2.kira.internal", "DNS_NAMES": "roll"})
    client1 = addDocker(net, 'c1', CLIENT_KIRA_IMAGE, "10.0.0.2", {"V4_TUNNEL_HOSTS": "s1.kira.internal"})
    client2 = addDocker(net, 'c2', CLIENT_KIRA_IMAGE, "10.0.0.3", {"V4_TUNNEL_HOSTS": "s1.kira.internal"})

    # some other kira nodes
    d4, d5, d6, d7 = [addDocker(net, f"d{i}", BASE_KIRA_IMAGE, f"10.0.0.{i}") for i in range(4, 8)]

    # add links
    net.addLink(server, d6)
    net.addLink(client1, d4)
    net.addLink(client2, d5)
    net.addLink(d4, d7)
    net.addLink(d4, d5)
    net.addLink(d5, d6)
    net.addLink(d7, d6)

def exec_node(node, cmd):
    info(f"Executing CMD on {node.name}: {cmd}")
    return node.popen(cmd, shell=True, stdout=PIPE).stdout.read(-1).decode()

if __name__ == "__main__":
    main()
