from collections import Counter

import networkx as nx
import pytest
from kira_nest.nest.cluster import (
    Cluster,
    ClusterError,
    assign_nodes,
    cross_host_edges,
    vxlan_ifname,
)

HOSTS = {
    "hosts": {
        "a": {"address": "192.0.2.1"},
        "b": {"address": "192.0.2.2"},
        "c": {"address": "192.0.2.3"},
    }
}


def ring(n: int) -> nx.Graph:
    return nx.relabel_nodes(nx.cycle_graph(n), str)


def cluster(local: str = "a", **extra) -> Cluster:
    return Cluster.from_dict({**HOSTS, **extra}, local)


def test_assignment_is_balanced_and_contiguous() -> None:
    graph = ring(30)
    assignment = assign_nodes(graph, cluster().hosts)

    assert set(assignment) == set(graph)
    assert Counter(assignment.values()) == {"a": 10, "b": 10, "c": 10}
    # breadth first parts of a ring: each part is connected
    for host in "abc":
        part = [tid for tid, h in assignment.items() if h == host]
        assert nx.is_connected(graph.subgraph(part))


def test_assignment_is_deterministic_regardless_of_insertion_order() -> None:
    graph = ring(20)
    shuffled = nx.Graph()
    shuffled.add_nodes_from(reversed(list(graph)))
    shuffled.add_edges_from(reversed(list(graph.edges())))

    hosts = cluster().hosts
    assert assign_nodes(graph, hosts) == assign_nodes(shuffled, hosts)


def test_gml_attribute_and_pinned_nodes_take_precedence() -> None:
    graph = ring(6)
    graph.nodes["0"]["host"] = "c"
    raw = {"hosts": {**HOSTS["hosts"], "b": {"address": "192.0.2.2", "nodes": ["3"]}}}
    assignment = assign_nodes(graph, Cluster.from_dict(raw, "a").hosts)

    assert assignment["0"] == "c"
    assert assignment["3"] == "b"
    assert Counter(assignment.values()) == {"a": 2, "b": 2, "c": 2}


def test_pinned_node_by_config_name() -> None:
    graph = ring(4)
    for tid in graph:
        graph.nodes[tid]["config"] = {"name": f"k{tid}"}
    raw = {"hosts": {"a": {"address": "192.0.2.1", "nodes": ["k2"]}}}
    assert assign_nodes(graph, Cluster.from_dict(raw, "a").hosts)["2"] == "a"


def test_unknown_host_or_node_is_rejected() -> None:
    graph = ring(4)
    graph.nodes["1"]["host"] = "z"
    with pytest.raises(ClusterError):
        assign_nodes(graph, cluster().hosts)

    raw = {"hosts": {"a": {"address": "192.0.2.1", "nodes": ["nope"]}}}
    with pytest.raises(ClusterError):
        assign_nodes(ring(4), Cluster.from_dict(raw, "a").hosts)


def test_invalid_cluster_files() -> None:
    with pytest.raises(ClusterError):
        Cluster.from_dict({}, "a")
    with pytest.raises(ClusterError):
        cluster(local="x")
    with pytest.raises(ClusterError):
        Cluster.from_dict({"hosts": {"a": {}}}, "a")
    with pytest.raises(ClusterError):
        Cluster.from_dict(
            {"hosts": {"a": {"address": "192.0.2.1"}, "b": {"address": "2001:db8::1"}}},
            "a",
        )


def test_cross_links_agree_between_hosts() -> None:
    graph = ring(30)
    graph.add_edge("0", "15")  # crosses from a to b
    by_host = {}
    for local in "abc":
        c = cluster(local, vni_base=500)
        c.assign(graph)
        by_host[local] = c.cross_links(graph)

    # every edge between hosts appears on both of its hosts with the same VNI
    ends: dict[int, set[tuple[str, str]]] = {}
    for local, links in by_host.items():
        for link in links:
            assert link.vni >= 500  # noqa: PLR2004
            assert link.remote_host.name != local
            ends.setdefault(link.vni, set()).add((local, link.remote_host.name))
    assert all(len(e) == 2 for e in ends.values())  # noqa: PLR2004

    c = cluster()
    c.assign(graph)
    assert len(ends) == len(cross_host_edges(graph, c.assignment))
    assert len(vxlan_ifname(2**24 - 1)) <= 15  # noqa: PLR2004
