"""Distribution of one emulated topology over several hosts.

Every host loads the same GML topology and the same cluster file and emulates only
the nodes assigned to it. Links between nodes on different hosts are VXLAN tunnels
between the hosts. All hosts derive the assignment of nodes and the VXLAN network
identifiers (VNIs) of the links from these two files alone, so they agree on them
without talking to each other.

A cluster file (TOML) looks like this::

    # underlay interface for the VXLAN tunnels (optional, default: routing decides)
    device = "eth0"
    # VNI of the first link between hosts (optional)
    vni_base = 1000
    # UDP port of VXLAN (optional)
    port = 4789
    # MTU of the VXLAN interfaces in the nodes (optional, default: the kernel's)
    mtu = 1450

    [hosts.alpha]
    address = "192.0.2.1"     # underlay address of the host

    [hosts.beta]
    address = "192.0.2.2"
    nodes = ["k7", "k8"]      # optional: nodes pinned to this host (labels or names)

A node is assigned to a host by its GML attribute `host`, otherwise by the `nodes`
list of a host, otherwise automatically. Every host gets an equal share of the nodes
(at most one node more than another host, unless pinned nodes demand more). The hosts
start from seeds far apart (or their pinned nodes) and grow their regions together,
each taking the free neighbor with the most links into its region. Then swaps of
nodes between hosts join pieces of regions and cut links between hosts, as long as
that keeps the sizes. The regions are connected if the topology allows that with
equal shares, which meshed topologies do, but not every tree.
"""

from __future__ import annotations

import logging
import math
import subprocess
import tomllib
from collections import Counter, deque
from collections.abc import Hashable, Iterable
from dataclasses import dataclass, field
from ipaddress import IPv4Address, IPv6Address, ip_address
from pathlib import Path
from typing import Any

import networkx as nx

logger = logging.getLogger(__name__)

# GML node attribute naming the host of a node
HOST_ATTRIBUTE = "host"

DEFAULT_VNI_BASE = 1000
DEFAULT_PORT = 4789
MAX_VNI = 2**24 - 1


class ClusterError(ValueError):
    pass


@dataclass(frozen=True)
class ClusterHost:
    name: str
    address: IPv4Address | IPv6Address
    nodes: tuple[str, ...] = ()


@dataclass(frozen=True)
class CrossLink:
    """A link between a node on this host and a node on another host."""

    local_tid: Hashable
    remote_tid: Hashable
    remote_host: ClusterHost
    vni: int


@dataclass(frozen=True)
class Cluster:
    hosts: dict[str, ClusterHost]
    local: str
    device: str | None = None
    vni_base: int = DEFAULT_VNI_BASE
    port: int = DEFAULT_PORT
    mtu: int | None = None
    # node -> host, filled by `assign`
    assignment: dict[Hashable, str] = field(default_factory=dict, compare=False)

    @classmethod
    def load(cls, path: Path, local: str) -> Cluster:
        with path.open("rb") as f:
            raw = tomllib.load(f)
        return cls.from_dict(raw, local)

    @classmethod
    def from_dict(cls, raw: dict[str, Any], local: str) -> Cluster:
        raw_hosts = raw.get("hosts")
        if not isinstance(raw_hosts, dict) or not raw_hosts:
            raise ClusterError("the cluster file defines no [hosts.<name>] tables")

        hosts: dict[str, ClusterHost] = {}
        for name, raw_host in raw_hosts.items():
            try:
                address = ip_address(raw_host["address"])
            except (KeyError, TypeError, ValueError) as e:
                raise ClusterError(f"host {name}: invalid or missing address") from e
            nodes = tuple(str(n) for n in raw_host.get("nodes", ()))
            hosts[name] = ClusterHost(name, address, nodes)

        if local not in hosts:
            raise ClusterError(
                f"this host '{local}' is not in the cluster file"
                f" (hosts: {', '.join(hosts)}); select it with --host"
            )
        families = {h.address.version for h in hosts.values()}
        if len(families) > 1:
            raise ClusterError("all host addresses must be of the same IP version")

        cluster = cls(
            hosts=hosts,
            local=local,
            device=raw.get("device"),
            vni_base=int(raw.get("vni_base", DEFAULT_VNI_BASE)),
            port=int(raw.get("port", DEFAULT_PORT)),
            mtu=int(raw["mtu"]) if "mtu" in raw else None,
        )
        if not 0 < cluster.vni_base <= MAX_VNI:
            raise ClusterError(f"vni_base must be in 1..{MAX_VNI}")
        return cluster

    @property
    def local_host(self) -> ClusterHost:
        return self.hosts[self.local]

    def assign(self, graph: nx.Graph) -> dict[Hashable, str]:
        """Assigns each node of the graph to a host (see the module documentation)."""
        self.assignment.clear()
        self.assignment.update(assign_nodes(graph, self.hosts))
        return self.assignment

    def host_of(self, tid: Hashable) -> str:
        return self.assignment[tid]

    def is_local(self, tid: Hashable) -> bool:
        return self.assignment[tid] == self.local

    def cross_links(self, graph: nx.Graph) -> list[CrossLink]:
        """The links of the graph between this host and others, with their VNIs."""
        links = []
        for vni, (x, y) in enumerate(cross_host_edges(graph, self.assignment)):
            vni += self.vni_base  # noqa: PLW2901
            if vni > MAX_VNI:
                raise ClusterError("too many links between hosts for vni_base")
            if self.is_local(x):
                local_tid, remote_tid = x, y
            elif self.is_local(y):
                local_tid, remote_tid = y, x
            else:
                continue
            remote_host = self.hosts[self.assignment[remote_tid]]
            links.append(CrossLink(local_tid, remote_tid, remote_host, vni))
        return links

    def create_vxlan(self, link: CrossLink, netns: str, address: str) -> str:
        """Creates the VXLAN interface of the link in the namespace `netns`.

        The interface is created in the namespace of this process, so its UDP socket
        stays there and reaches the other host over the underlay, and then moved into
        `netns`. Returns the name of the interface.
        """
        ifname = vxlan_ifname(link.vni)
        # an interface left over by an aborted run would block the name
        subprocess.run(
            ["ip", "link", "del", ifname],
            check=False,
            capture_output=True,
        )
        cmd = [
            "ip", "link", "add", ifname, "type", "vxlan",
            "id", str(link.vni),
            "local", str(self.local_host.address),
            "remote", str(link.remote_host.address),
            "dstport", str(self.port),
            "nolearning",
        ]  # fmt: skip
        if self.device:
            cmd += ["dev", self.device]
        _run(cmd)
        _run(["ip", "link", "set", ifname, "netns", netns])
        if self.mtu is not None:
            _run(["ip", "-n", netns, "link", "set", ifname, "mtu", str(self.mtu)])
        _run(["ip", "-n", netns, "address", "add", address, "dev", ifname])
        _run(["ip", "-n", netns, "link", "set", ifname, "up"])
        return ifname

    def describe(self) -> str:
        counts = {name: 0 for name in self.hosts}
        for host in self.assignment.values():
            counts[host] += 1
        return ", ".join(
            f"{name}{'*' if name == self.local else ''}"
            f" ({self.hosts[name].address}): {counts[name]} nodes"
            for name in self.hosts
        )


def vxlan_ifname(vni: int) -> str:
    return f"kvx{vni}"  # at most 11 characters of the 15 Linux allows


def _run(cmd: list[str]) -> None:
    result = subprocess.run(cmd, check=False, capture_output=True, text=True)
    if result.returncode != 0:
        raise RuntimeError(f"'{' '.join(cmd)}' failed: {result.stderr.strip()}")


def _sort_key(tid: Hashable) -> tuple[str, int | str]:
    """Orders labels like `2` < `10` and `k2` < `k10`, the same on every host."""
    s = str(tid)
    head = s.rstrip("0123456789")
    tail = s[len(head) :]
    return (head, int(tail) if tail else -1)


def _match_node(graph: nx.Graph, key: str) -> Hashable | None:
    """Finds a node by its label or by the name in its config."""
    for tid in graph:
        if str(tid) == key:
            return tid
    for tid, cfg in graph.nodes(data="config"):
        name = cfg.get("name") if isinstance(cfg, dict) else getattr(cfg, "name", None)
        if name == key:
            return tid
    return None


def assign_nodes(graph: nx.Graph, hosts: dict[str, ClusterHost]) -> dict[Hashable, str]:
    """Assigns each node to a host, deterministically for the same inputs."""
    assignment: dict[Hashable, str] = {}

    for tid, host in graph.nodes(data=HOST_ATTRIBUTE):
        if host is None:
            continue
        if host not in hosts:
            raise ClusterError(f"node {tid} is assigned to unknown host '{host}'")
        assignment[tid] = host

    for host in hosts.values():
        for key in host.nodes:
            tid = _match_node(graph, key)
            if tid is None:
                raise ClusterError(f"host {host.name}: unknown node '{key}'")
            if assignment.setdefault(tid, host.name) != host.name:
                raise ClusterError(
                    f"node {key} is assigned to {assignment[tid]} and {host.name}"
                )

    pinned = set(assignment)
    order = {tid: i for i, tid in enumerate(sorted(graph, key=_sort_key))}
    targets = _targets(graph.number_of_nodes(), hosts, Counter(assignment.values()))
    _place_seeds(graph, assignment, targets, order)
    _grow_regions(graph, assignment, targets, order)
    _refine(graph, assignment, pinned, order)
    return assignment


def _targets(n: int, hosts: dict[str, ClusterHost], pinned: Counter) -> dict[str, int]:
    """Number of nodes per host: equal shares, but at least the pinned nodes."""
    base, extra = divmod(n, len(hosts))
    targets = {
        name: max(base + (i < extra), pinned[name]) for i, name in enumerate(hosts)
    }
    # pinned nodes beyond a share are taken from the shares of the largest others
    while sum(targets.values()) > n:
        name = max(
            (h for h in hosts if targets[h] > pinned[h]),
            key=lambda h: targets[h] - pinned[h],
        )
        targets[name] -= 1
    return targets


def _distances(graph: nx.Graph, sources: Iterable[Hashable]) -> dict[Hashable, int]:
    """Hop counts from the nearest source (breadth first)."""
    dist = dict.fromkeys(sources, 0)
    queue = deque(dist)
    while queue:
        tid = queue.popleft()
        for neighbor in graph[tid]:
            if neighbor not in dist:
                dist[neighbor] = dist[tid] + 1
                queue.append(neighbor)
    return dist


def _place_seeds(
    graph: nx.Graph,
    assignment: dict[Hashable, str],
    targets: dict[str, int],
    order: dict[Hashable, int],
) -> None:
    """Seeds each host without pinned nodes with a node far from all other regions.

    Nodes unreachable from the regions count as farthest, so further components of
    the graph get seeds of their own.
    """
    regions = set(assignment.values())
    for name, target in targets.items():
        if name in regions or target == 0:
            continue
        # without any region yet, start at the periphery
        sources = assignment or [min(order, key=order.__getitem__)]
        dist = _distances(graph, sources)
        seed = max(
            (tid for tid in graph if tid not in assignment),
            key=lambda t: (dist.get(t, math.inf), -order[t]),
        )
        assignment[seed] = name


def _grow_regions(
    graph: nx.Graph,
    assignment: dict[Hashable, str],
    targets: dict[str, int],
    order: dict[Hashable, int],
) -> None:
    """Grows the regions of all hosts together until they reach their targets.

    The host furthest from its target takes next. It takes the free neighbor of its
    region with the most links into the region, which keeps the regions compact.
    """
    hosts = list(targets)
    load: Counter = Counter()
    # free neighbors of each region and their numbers of links into it
    frontier: dict[str, dict[Hashable, int]] = {name: {} for name in hosts}

    def take(tid: Hashable, name: str) -> None:
        assignment[tid] = name
        load[name] += 1
        for f in frontier.values():
            f.pop(tid, None)
        for neighbor in graph[tid]:
            if neighbor not in assignment:
                frontier[name][neighbor] = frontier[name].get(neighbor, 0) + 1

    for tid, name in list(assignment.items()):
        take(tid, name)

    while len(assignment) < graph.number_of_nodes():
        name = min(
            (h for h in hosts if load[h] < targets[h]),
            key=lambda h: (load[h] / targets[h], hosts.index(h)),
        )
        if frontier[name]:
            f = frontier[name]
            tid = max(f, key=lambda t: (f[t], -order[t]))
        else:
            # enclosed by other regions: the nearest free node, or any free one
            dist = _distances(graph, [t for t, h in assignment.items() if h == name])
            tid = min(
                (t for t in graph if t not in assignment),
                key=lambda t: (dist.get(t, math.inf), order[t]),
            )
        take(tid, name)


def _refine(
    graph: nx.Graph,
    assignment: dict[Hashable, str],
    pinned: set[Hashable],
    order: dict[Hashable, int],
) -> None:
    """Swaps pairs of boundary nodes between hosts to join and compact the parts.

    Like Kernighan-Lin, but greedy. A swap must reduce the stray nodes (nodes outside
    the largest connected piece of their part) or, with as many stray nodes, the
    links between hosts. Each swap reduces (stray nodes, cut links), so it ends.
    The sizes of the parts stay the same, and pinned nodes stay where they are.
    """
    parts: dict[str, set[Hashable]] = {}
    for tid, name in assignment.items():
        parts.setdefault(name, set()).add(tid)
    stray = {name: _stray(graph, nodes) for name, nodes in parts.items()}

    while True:
        for key, u, v in _swap_candidates(graph, assignment, pinned, order, stray):
            gain = -key[1]
            a, b = assignment[u], assignment[v]
            new_a = parts[a] - {u} | {v}
            new_b = parts[b] - {v} | {u}
            stray_a, stray_b = _stray(graph, new_a), _stray(graph, new_b)
            before, after = stray[a] + stray[b], stray_a + stray_b
            if after < before or (after == before and gain > 0):
                parts[a], parts[b] = new_a, new_b
                stray[a], stray[b] = stray_a, stray_b
                assignment[u], assignment[v] = b, a
                break
        else:
            return


def _stray(graph: nx.Graph, nodes: set[Hashable]) -> int:
    """Number of nodes outside the largest connected piece of the nodes."""
    largest = max(len(c) for c in nx.connected_components(graph.subgraph(nodes)))
    return len(nodes) - largest


def _swap_candidates(
    graph: nx.Graph,
    assignment: dict[Hashable, str],
    pinned: set[Hashable],
    order: dict[Hashable, int],
    stray: dict[str, int],
) -> list[tuple[tuple[int, int, int], Hashable, Hashable]]:
    """Swaps of boundary nodes u and v between two hosts, the most promising first.

    Swaps out of parts with stray nodes come first, then by links saved. Swaps
    saving no links are only included between parts with stray nodes.
    """

    def links(tid: Hashable, name: str) -> int:
        return sum(1 for n in graph[tid] if assignment[n] == name)

    # links saved by moving boundary node u from host a to host b
    moves: dict[tuple[str, str], list[tuple[int, Hashable]]] = {}
    for u in sorted(graph, key=order.__getitem__):
        if u in pinned:
            continue
        a = assignment[u]
        for b in sorted({assignment[n] for n in graph[u]} - {a}):
            moves.setdefault((a, b), []).append((links(u, b) - links(u, a), u))

    swaps = []
    for (a, b), moves_ab in moves.items():
        if a > b:
            continue
        defragment = stray[a] + stray[b] > 0
        for gain_u, u in moves_ab:
            for gain_v, v in moves.get((b, a), ()):
                gain = gain_u + gain_v - 2 * graph.has_edge(u, v)
                if gain > 0 or defragment:
                    swaps.append(((-defragment, -gain, order[u] + order[v]), u, v))
    swaps.sort(key=lambda s: s[0])
    return swaps


def cross_host_edges(
    graph: nx.Graph, assignment: dict[Hashable, str]
) -> list[tuple[Hashable, Hashable]]:
    """The edges between nodes of different hosts, in the same order on every host."""
    edges = []
    for x, y in graph.edges():
        if assignment[x] != assignment[y]:
            x, y = sorted((x, y), key=_sort_key)  # noqa: PLW2901
            edges.append((x, y))
    edges.sort(key=lambda e: (_sort_key(e[0]), _sort_key(e[1])))
    return edges
