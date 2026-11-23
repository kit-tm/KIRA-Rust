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
list of a host, otherwise automatically: in the order of the cluster file, each host
grows a connected region of the remaining nodes (breadth first from its pinned nodes
or the first free node) until it has an equal share of all nodes. So all hosts
emulate about equally many nodes and few links cross hosts.
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

    # grow a region of the remaining nodes for each host in the order of the cluster
    # file, breadth first from the hosts' pinned nodes or else from the first free
    # node, until each host has an equal share of all nodes
    share = math.ceil(graph.number_of_nodes() / len(hosts))
    load = Counter(assignment.values())
    for name in hosts:
        seeds = sorted(
            (tid for tid, host in assignment.items() if host == name), key=_sort_key
        )
        for tid in _grow_region(graph, assignment, seeds, share - load[name]):
            assignment[tid] = name
            load[name] += 1
    # pinned nodes exceeded the shares: the least loaded hosts take the rest
    for tid in sorted(graph, key=_sort_key):
        if tid not in assignment:
            name = min(hosts, key=lambda h: load[h])
            assignment[tid] = name
            load[name] += 1
    return assignment


def _grow_region(
    graph: nx.Graph,
    assigned: dict[Hashable, str],
    seeds: list[Hashable],
    size: int,
) -> Iterable[Hashable]:
    """Yields up to `size` unassigned nodes in breadth first order from the seeds.

    When the region can't grow anymore, it continues from the first unassigned node.
    """
    taken: set[Hashable] = set()
    order = sorted(graph, key=_sort_key)
    queue = deque(seeds)
    seen = set(seeds)
    while len(taken) < size:
        if not queue:
            start = next(
                (t for t in order if t not in assigned and t not in taken), None
            )
            if start is None:
                return
            queue.append(start)
            seen.add(start)
        tid = queue.popleft()
        if tid not in assigned:
            taken.add(tid)
            yield tid
        for neighbor in sorted(graph[tid], key=_sort_key):
            if neighbor not in seen and neighbor not in assigned:
                seen.add(neighbor)
                queue.append(neighbor)


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
