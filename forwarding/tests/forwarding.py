from mininet.cli import CLI
from mininet.net import Containernet
from mininet.node import Controller
from mininet.log import info, setLogLevel
from time import sleep

setLogLevel('info')

SYSCTL = {'net.ipv6.conf.all.disable_ipv6': 0, 'net.ipv6.conf.all.forwarding': 1}

net = Containernet(controller=Controller)
d1 = net.addDocker(name="d1", sysctls=SYSCTL, cap_add=['net_admin'], dimage="test")
d2 = net.addDocker(name="d2", sysctls=SYSCTL, cap_add=['net_admin'], dimage="test")
d3 = net.addDocker(name="d3", sysctls=SYSCTL, cap_add=['net_admin'], dimage="test")

net.addLink(d1, d2)
net.addLink(d2, d3)

net.start()

info(d1.cmd("ip a add fc00::1/16 dev d1-eth0"))
info(d1.cmd("./start.sh"))
info(d1.cmd("./map.sh fcaa::1 fc00::1"))
info(d1.cmd("./nodetopath.sh fc00::3 fcaa::23"))
info(d1.cmd("./routevia.sh fcaa::23 fc00::2"))

info(d2.cmd("ip a add fc00::2/16 dev d2-eth0"))
info(d2.cmd("ip a add fc00::2/16 dev d2-eth1"))
info(d2.cmd("ip route add fc00::3/128 dev d2-eth1"))
info(d2.cmd("ip route add fc00::1/128 dev d2-eth0"))
info(d2.cmd("./start.sh"))
info(d2.cmd("./map.sh fcaa::23 fcaa::3"))
info(d2.cmd("./map.sh fcaa::21 fcaa::1"))
info(d2.cmd("./routevia.sh fcaa::1 fc00::1"))
info(d2.cmd("./routevia.sh fcaa::3 fc00::3"))

info(d3.cmd("ip a add fc00::3/16 dev d3-eth0"))
info(d3.cmd("./start.sh"))
info(d3.cmd("./map.sh fcaa::3 fc00::3"))
info(d3.cmd("./nodetopath.sh fc00::1 fcaa::21"))
info(d3.cmd("./routevia.sh fcaa::21 fc00::2"))

CLI(net)

net.stop()
