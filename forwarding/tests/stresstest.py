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

# Create a topology like this:
#
# 
# 
# 
# 
# 
# 
# 
matrix = []

for i in range(10):
    matrix.append([add_node((i+1)*10)])
    for j in range(9):
        matrix[i].append(add_node(((i+1)*10) + j + 1))

for i in range(9):
    net.addLink(matrix[i][0], matrix[i+1][0])
for i in range(10):
    for j in range(9):
        net.addLink(matrix[i][j], matrix[i][j+1])

for i in range(10):
    for j in range(10):
        index = ((i+1)*10)+j
        node = matrix[i][j]
        node.cmd(f'ip a add fc00::{index}/128 dev d{index}-eth0')
        node.cmd(f'ip a add fc00::{index}/128 dev d{index}-eth1')
        node.cmd(f'ip a add fc00::{index}/128 dev d{index}-eth2')

for row in matrix:
    for node in row:
        node.cmd("touch /r2kad-daemon/start")

net.start()

CLI(net)

net.stop()
