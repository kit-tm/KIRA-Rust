from collections import Counter
from pathlib import Path

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


def hosts(n: int) -> dict:
    raw = {f"h{i}": {"address": f"192.0.2.{i + 1}"} for i in range(n)}
    return Cluster.from_dict({"hosts": raw}, "h0").hosts


def as_str(graph: nx.Graph) -> nx.Graph:
    return nx.relabel_nodes(nx.convert_node_labels_to_integers(graph), str)


TOPOS = Path(__file__).resolve().parents[5] / "topos"

# meshed topologies like those of the tests, which can be split into connected parts
GRAPHS = {
    "minimal": nx.read_gml(TOPOS / "minimal.gml"),
    "large": nx.read_gml(TOPOS / "large.gml"),
    "large-100": nx.read_gml(TOPOS / "large-100.gml"),
    "grid": as_str(nx.grid_2d_graph(12, 12)),
    "small-world": as_str(nx.connected_watts_strogatz_graph(200, 6, 0.1, seed=1)),
}
# a tree can't always be split into equal connected parts, nor can the greedy
# refinement always split a barbell so
HARD_GRAPHS = {
    "tree": as_str(nx.balanced_tree(3, 4)),
    "barbell": as_str(nx.barbell_graph(15, 10)),
}


def assert_balanced(assignment: dict, n_hosts: int) -> Counter:
    sizes = Counter(assignment.values())
    assert len(sizes) == n_hosts
    assert max(sizes.values()) - min(sizes.values()) <= 1
    return sizes


@pytest.mark.parametrize("name", GRAPHS)
@pytest.mark.parametrize("n_hosts", [2, 3, 4, 6, 7])
def test_every_part_is_connected_and_balanced(name: str, n_hosts: int) -> None:
    graph = GRAPHS[name]
    assignment = assign_nodes(graph, hosts(n_hosts))

    sizes = assert_balanced(assignment, n_hosts)
    for host in sizes:
        part = [tid for tid, h in assignment.items() if h == host]
        assert nx.is_connected(graph.subgraph(part)), (host, sizes)


@pytest.mark.parametrize("name", HARD_GRAPHS)
@pytest.mark.parametrize("n_hosts", [2, 4, 7])
def test_balance_is_kept_without_connected_split(name: str, n_hosts: int) -> None:
    assert_balanced(assign_nodes(HARD_GRAPHS[name], hosts(n_hosts)), n_hosts)


def test_no_host_is_left_empty() -> None:
    # equal shares rounded up (20 / 6 -> 4) would leave the last host empty
    assignment = assign_nodes(ring(20), hosts(6))
    assert sorted(Counter(assignment.values()).values()) == [3, 3, 3, 3, 4, 4]


def test_cut_is_small_on_a_grid() -> None:
    graph = GRAPHS["grid"]
    # two halves of a 12x12 grid are separated by 12 links, any split needs that
    cut = cross_host_edges(graph, assign_nodes(graph, hosts(2)))
    assert len(cut) <= 20  # noqa: PLR2004


def test_more_pinned_nodes_than_a_share() -> None:
    graph = ring(12)
    raw = {
        "hosts": {
            "a": {"address": "192.0.2.1", "nodes": [str(i) for i in range(8)]},
            "b": {"address": "192.0.2.2"},
            "c": {"address": "192.0.2.3"},
        }
    }
    assignment = assign_nodes(graph, Cluster.from_dict(raw, "a").hosts)
    assert all(assignment[str(i)] == "a" for i in range(8))
    assert Counter(assignment.values()) == {"a": 8, "b": 2, "c": 2}


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
