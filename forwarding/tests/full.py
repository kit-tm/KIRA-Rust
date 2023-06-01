from mininet.cli import CLI
from mininet.net import Containernet
from mininet.node import Controller
from mininet.log import info, setLogLevel
from time import sleep

setLogLevel('info')

SYSCTL = {'net.ipv6.conf.all.disable_ipv6': 0, 'net.ipv6.conf.all.forwarding': 1}

net = Containernet(controller=Controller)
d1 = net.addDocker(name="d1", sysctls=SYSCTL, cap_add=['net_admin'], dimage="r2kad-daemon:full", ip='fc00::1', dcmd="/r2kad-daemon/r2kad-daemon", environment={"NODE_ID": f'{1:028}', "RUST_LOG": "info"})
d2 = net.addDocker(name="d2", sysctls=SYSCTL, cap_add=['net_admin'], dimage="r2kad-daemon:full", ip='fc00::2', dcmd="/r2kad-daemon/r2kad-daemon", environment={"NODE_ID": f'{2:028}', "RUST_LOG": "info"})
#d3 = net.addDocker(name="d3", sysctls=SYSCTL, cap_add=['net_admin'], dimage="r2kad-daemon:full", dcmd="/r2kad-daemon/r2kad-daemon", environment={"NODE_ID": f'{3:028}'})

net.addLink(d1, d2)
#net.addLink(d2, d3)

net.start()

#info(d1.cmd("ip a add fc00::1/16 dev d1-eth0"))
#info(d1.cmd("/r2kad-daemon/r2kad-daemon"))
#info(d2.cmd("ip a add fc00::2/16 dev d2-eth0"))
#info(d2.cmd("/r2kad-daemon/r2kad-daemon"))

#info(d1.cmd("./start.sh fc00::1"))
#info(d1.cmd("./nodetopath.sh fc00::3 fcaa:1::b"))
#info(d1.cmd("ip a add fc00::1/16 dev d1-eth0"))
#info(d1.cmd("ip a add fcaa:1::a/32 dev d1-eth0"))

#info(d2.cmd("./start.sh fc00::2"))
#info(d2.cmd("./pathtopath.sh fcaa:1::b fcaa:2::c"))
#info(d2.cmd("./pathtopath.sh fcaa:2::b fcaa:1::a"))
#info(d2.cmd("ip a add fcaa:1::b/32 dev d2-eth0"))
#info(d2.cmd("ip a add fcaa:2::b/32 dev d2-eth1"))

#info(d3.cmd("./start.sh fc00::3"))
#info(d3.cmd("./nodetopath.sh fc00::1 fcaa:2::b"))
#info(d3.cmd("ip a add fc00::3/16 dev d3-eth0"))
#info(d3.cmd("ip a add fcaa:2::c/32 dev d3-eth0"))

CLI(net)

net.stop()
