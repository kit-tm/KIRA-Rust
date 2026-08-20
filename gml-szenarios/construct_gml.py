from kira_net import NodeConfig, DefaultNodeConfig

from networkx import Graph
import networkx as nx

SEED = 1234

default = DefaultNodeConfig(
    name="default",
    #docker_image="6gdemo-base:latest",
    docker_image="kirad",
    dns=["127.0.0.1"],
    # sysctls={'net.ipv6.conf.eth0.disable_ipv6': 0},
    debug_level="info",
    seed=SEED
)
agent_default = DefaultNodeConfig(
    name="agent_default",
    #docker_image="kira:agent",
    docker_image="kirad",
    dns=["127.0.0.1"],
    # sysctls={'net.ipv6.conf.eth0.disable_ipv6': 0},
    debug_level="debug",
    seed=SEED,
    enviroments={"V4_TUNNEL_HOSTS": "sim1.kira.internal"}
)

# node specific configs
sim1_conf = NodeConfig(
    name="sim1",
    #docker_image="kira:sim",
    docker_image="kirad",
    ip_v4="10.0.0.1",
    enviroments={
        "V4_TUNNEL_HOSTS": "agent1.kira.internal:agent2.kira.internal"
    },
    default=default,
)

agent1_conf = NodeConfig(
    name="agent1",
    ip_v4="10.0.0.2",
    default=agent_default,
)

agent2_conf = NodeConfig(
    name="agent2",
    ip_v4="10.0.0.3",
    default=agent_default,
)

d4_conf, d5_conf, d6_conf, d7_conf = [NodeConfig(
    ip_v4=f"10.0.0.{i}", default=default, name=f"d{i}") for i in range(4, 8)]


# generate graph
G = Graph()

G.add_node("sim1")
G.add_node("agent1")
G.add_node("agent2")

for i in range(4, 8):
    G.add_node(f"d{i}")

G.add_edge("sim1", "d6")
G.add_edge("agent1", "d4")
G.add_edge("agent2", "d5")
G.add_edge("agent2", "d7")
G.add_edge("d4", "d5")
G.add_edge("d5", "d6")
G.add_edge("d7", "d6")


# save configs in Graph
default.save_in_graph(G)
agent_default.save_in_graph(G)

sim1_conf.save_in_graph(G, "sim1")
agent1_conf.save_in_graph(G, "agent1")
agent2_conf.save_in_graph(G, "agent2")
for i in range(4, 8):
    eval(f"d{i}_conf.save_in_graph(G, \"d{i}\")")

nx.write_gml(G, "test.gml")
