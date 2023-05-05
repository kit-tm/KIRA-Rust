from mininet.cli import CLI
from mininet.net import Containernet
from mininet.node import Controller
from mininet.log import info, setLogLevel
from time import sleep

# This is a simple test for encapsulation. 2 nodes are configured.
# Node d1 has nodeID fc00::1 and uses pathID fcaa::2 for destination fc00::2.
# Node d2 has nodeID fc00::2 and uses pathID fcaa::1 for destination fc00::1.

# All traffic from fc00::1 -> fc00::2 is first encapsulated on d1:
# ping fc00::2 -> [fc00::1 -> fc00::2 (ping)] -> kira -> 
# -> [fc00::1 -> fcaa::2 ([fc00::1 -> fc00::2 (ping)])] -> d1-eth0

setLogLevel('info')

SYSCTL = {'net.ipv6.conf.all.disable_ipv6': 0, 'net.ipv6.conf.all.forwarding':1}

net = Containernet(controller=Controller)
d1 = net.addDocker(name="d1", ip='aaaa::1/16', sysctls=SYSCTL, cap_add=['net_admin'], dimage="test")
d2 = net.addDocker(name="d2", ip='aaaa::2/16', sysctls=SYSCTL, cap_add=['net_admin'], dimage="test")
net.addLink(d1, d2)

net.start()

info(d1.cmd("./start.sh fc00::1"))
info(d1.cmd("./localpath.sh fcaa::1"))
info(d1.cmd("./nodetopath.sh fc00::2 fcaa::2"))
info(d1.cmd("ip a add fc00::1/16 dev d1-eth0"))
info(d1.cmd("ip a add fcaa::1/16 dev d1-eth0"))
info(d1.cmd("ip a add beef::a/16 dev d1-eth0"))

# info(d1.cmd("ip a add beef::a/16 dev d1-eth0"))

info(d2.cmd("./start.sh fc00::2"))
info(d2.cmd("./localpath.sh fcaa::2"))
info(d2.cmd("./nodetopath.sh fc00::1 fcaa::1"))
info(d2.cmd("ip a add fc00::2/16 dev d2-eth0"))
info(d2.cmd("ip a add fcaa::2/16 dev d2-eth0"))
info(d2.cmd("ip a add beef::a/16 dev d2-eth0"))

CLI(net)

net.stop()
