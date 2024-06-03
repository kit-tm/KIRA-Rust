from networkx import Graph
import networkx as nx

from mininet.net import Containernet, Controller
from mininet.cli import CLI

from kira_net.network import Network

G = nx.read_gml("test.gml")

net = Containernet(controller=Controller)
Network(G, net)
try:
    net.start()
    CLI(net)
finally:
    net.stop()
