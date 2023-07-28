from mininet.cli import CLI
from mininet.net import Containernet
from mininet.node import Controller
from mininet.log import info, setLogLevel
from time import sleep

setLogLevel('info')

SYSCTL = {'net.ipv6.conf.all.disable_ipv6': 0, 'net.ipv6.conf.all.forwarding': 1}

net = Containernet(controller=Controller)
d1 = net.addDocker(name="d1", sysctls=SYSCTL, cap_add=['net_admin'], dimage="r2kad-daemon:full", dcmd="bash /r2kad-daemon/start.sh", environment={"NODE_ID": f'{1:028}', "RUST_LOG": "trace"})
d2 = net.addDocker(name="d2", sysctls=SYSCTL, cap_add=['net_admin'], dimage="r2kad-daemon:full", dcmd="bash /r2kad-daemon/start.sh", environment={"NODE_ID": f'{2:028}', "RUST_LOG": "trace"})
d3 = net.addDocker(name="d3", sysctls=SYSCTL, cap_add=['net_admin'], dimage="r2kad-daemon:full", dcmd="bash /r2kad-daemon/start.sh", environment={"NODE_ID": f'{3:028}', "RUST_LOG": "trace"})

net.addLink(d1, d2)
net.addLink(d2, d3)

info(d1.cmd("ip a add fc00::1/128 dev d1-eth0"))
info(d2.cmd("ip a add fc00::2/128 dev d2-eth0"))
info(d2.cmd("ip a add fc00::2/128 dev d2-eth1"))
info(d3.cmd("ip a add fc00::3/128 dev d3-eth0"))

net.start()

CLI(net)

net.stop()
