from mininet.cli import CLI
from mininet.net import Containernet
from mininet.node import Controller
from mininet.log import info, setLogLevel
from time import sleep

setLogLevel('info')

SYSCTL = {'net.ipv6.conf.all.disable_ipv6': 0, 'net.ipv6.conf.all.forwarding': 1}

def add_node(index):
    return net.addDocker(name=f'd{index}', sysctls=SYSCTL, cap_add=['net_admin'], dimage="r2kad-daemon:full", dcmd="bash /r2kad-daemon/start.sh", environment={"NODE_ID": f'{index:028}', "RUST_LOG": "trace"})

net = Containernet(controller=Controller)
d1 = add_node(1)
d2 = add_node(2)
d3 = add_node(3)
d4 = add_node(4)
d5 = add_node(5)
d6 = add_node(6)

net.addLink(d1, d2)
net.addLink(d2, d3)
net.addLink(d2, d4)
net.addLink(d4, d5)
net.addLink(d4, d6)

info(d1.cmd("ip a add fc00::1/128 dev d1-eth0"))
info(d2.cmd("ip a add fc00::2/128 dev d2-eth0"))
info(d2.cmd("ip a add fc00::2/128 dev d2-eth1"))
info(d2.cmd("ip a add fc00::2/128 dev d2-eth2"))
info(d3.cmd("ip a add fc00::3/128 dev d3-eth0"))
info(d4.cmd("ip a add fc00::4/128 dev d4-eth0"))
info(d4.cmd("ip a add fc00::4/128 dev d4-eth1"))
info(d4.cmd("ip a add fc00::4/128 dev d4-eth2"))
info(d5.cmd("ip a add fc00::5/128 dev d5-eth0"))
info(d6.cmd("ip a add fc00::6/128 dev d6-eth0"))

d1.cmd("touch /r2kad-daemon/start")
d2.cmd("touch /r2kad-daemon/start")
d3.cmd("touch /r2kad-daemon/start")
d4.cmd("touch /r2kad-daemon/start")
d5.cmd("touch /r2kad-daemon/start")
d6.cmd("touch /r2kad-daemon/start")

net.start()

CLI(net)

net.stop()
