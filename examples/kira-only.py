#!/usr/bin/env python3
from mininet.net import Containernet
from mininet.cli import CLI
from mininet.log import info, warn, setLogLevel

setLogLevel('info')

KIRA_IMAGE = "kira:latest"

DCMD = "/usr/bin/supervisord -c /etc/supervisord.conf"
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


def setup(net):
    def addDocker(net, name, image):
        return net.addDocker(name, dimage=image, sysctls=SYSCTLS, ip=None, network_mode="none", dcmd=DCMD)

    info('*** Adding docker containers\n')
    # kira
    d1, d2, d3, d4, d5, d6, d7 = [addDocker(net, f"d{i}", KIRA_IMAGE) for i in range(1, 8)]

    # add links
    net.addLink(d1, d6)
    net.addLink(d2, d4)
    net.addLink(d3, d5)
    net.addLink(d3, d7)
    net.addLink(d4, d5)
    net.addLink(d5, d6)
    net.addLink(d7, d6)

if __name__ == "__main__":
    main()
