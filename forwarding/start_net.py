from mininet.cli import CLI
from mininet.net import Containernet
from mininet.node import Controller
from time import sleep

SYSCTL = {'net.ipv6.conf.all.disable_ipv6': 0}

net = Containernet(controller=Controller)
d1 = net.addDocker(name="d1", ip=None, sysctls=SYSCTL, dimage="test")
d2 = net.addDocker(name="d2", ip=None, sysctls=SYSCTL, dimage="test")
net.addLink(d1, d2)

net.start()

CLI(net)

net.stop()
