"""Data-plane notification about failed links.

After a link failed, the nodes whose precomputed paths contain the link remove the
forwarding entries of their PathIPs: the node at the link and the nodes upstream that
know the link from their vicinity. A packet reaching one of them is dropped there, and
the node notifies the node that encapsulated the packet (SegmentFailure), which then
switches to another path at once instead of detecting the failure by probing.

The notification is only needed if no other mechanism handles the failure first, so the
failed link is chosen such that
- the node encapsulating packets over the link (the source or an overlay hop on the way)
  learns nothing about it through the vicinity discovery: the link is outside of its
  vicinity,
- the node at the link has no detour to the next hop within its vicinity (fast vicinity
  alternative), whose links it knows up to VICINITY_RADIUS + 1 hops away, and
- its far end is neither the destination nor the end of the overlay hop over it,
  as the rediscovery of the lost neighbor by the node at the link may reach the
  encapsulating node otherwise (not-via data of the failed link).

The test is skipped if the topology has no such link on a path between two nodes.
"""

import contextlib
import io
import ipaddress
import os
import random
import re
import subprocess
import time

import networkx as nx
import pytest
from kira_nest.nest.test import KIRATest

# ping interval while the link fails
PING_INTERVAL_SECS = 0.1
PING_COUNT = 60
# pings sent before the link fails
PINGS_BEFORE_FAILURE = 10
# longest acceptable outage: well below the detection of the failure by probing
MAX_OUTAGE_SECS = 2.0
# radius of the vicinity in hops (VICINITY_RADIUS of kira-r2kad)
VICINITY_RADIUS = 2


def _traversed_links(test, src, dst, names) -> list | None:
    """Links a packet from `src` to `dst` traverses (traceroute), as tuples of their
    topology ids, the one of the node that encapsulated the packet for the overlay hop
    over the link and the one of the node where this overlay hop ends.
    """
    out = io.StringIO()
    with contextlib.redirect_stdout(out):
        if not test.traceroute(src, dst, verbose=True):
            return None
    links = []
    overlay_hop = []  # links of the current overlay hop
    hop = encapsulator = None
    for line in out.getvalue().splitlines():
        m = re.match(r"^(\S+)\s+:\s+(\S+)(?:.*-->\s+(\S+))?", line)
        if not m or m.group(1) not in names:
            continue
        node = names[m.group(1)]
        if hop is not None and node != hop:
            overlay_hop.append((hop, node, encapsulator))
        hop = node
        action, popped_to = m.group(2), m.group(3)
        if action == "PUSH":
            encapsulator = node
        # the overlay hop ends where the packet is decapsulated to its NodeIP (with
        # SRv6, the end of the first path segment reveals the PathIP of the second one)
        decapsulated = action == "POP" and popped_to and _is_node_ip(test, popped_to)
        if decapsulated or action == "ACK":
            links += [(x, y, e, node) for x, y, e in overlay_hop]
            overlay_hop = []
    return links


def _is_node_ip(test, address: str) -> bool:
    try:
        return ipaddress.IPv6Address(address) in test.ip_prefixes.node_ip
    except ValueError:
        return False


def _candidate_links(graph) -> dict:
    """Links whose failure only the notification reports quickly, with the nodes that
    may encapsulate packets over them: the network stays connected without the link,
    the node at the link has no detour within its vicinity and the link is outside the
    vicinity of the encapsulating node.
    """
    candidates = {}
    for x, y in graph.edges:
        reduced = graph.copy()
        reduced.remove_edge(x, y)
        if not nx.is_connected(reduced):
            continue
        if nx.shortest_path_length(reduced, x, y) <= VICINITY_RADIUS + 1:
            continue
        dist_x = nx.single_source_shortest_path_length(graph, x)
        dist_y = nx.single_source_shortest_path_length(graph, y)
        sources = {n for n in graph if min(dist_x[n], dist_y[n]) > VICINITY_RADIUS}
        if sources:
            candidates[frozenset((x, y))] = sources
    return candidates


def _choose_failure(test):
    """A pair of nodes, a candidate link of the path between them (see
    `_candidate_links`) and the node encapsulating packets over it, or None.

    The far end of the link is neither the destination nor the end of the overlay hop:
    the node at the link starts a rediscovery of its lost neighbor, whose requests
    carry the failed link (not-via) and reach nodes that have the neighbor as contact,
    possibly the encapsulating node.
    """
    candidates = _candidate_links(test.topology.topology)
    nodes = list(test.topology.nodes)
    names = {n.name: test.topology.tid(n) for n in nodes}
    pairs = [(u, v) for u in nodes for v in nodes if u != v]
    random.shuffle(pairs)
    for src, dst in pairs:
        links = _traversed_links(test, src, dst, names) or []
        for x, y, encapsulator, overlay_hop_end in links:
            if y in (names[dst.name], overlay_hop_end):
                continue
            if encapsulator in candidates.get(frozenset((x, y)), ()):
                hops = [links[0][0]] + [link[1] for link in links]
                return src, dst, hops, (x, y), encapsulator
    return None


def _longest_outage(output: str, count: int) -> int:
    """Longest run of consecutive pings without reply."""
    received = {int(s) for s in re.findall(r"bytes from .* icmp_seq=(\d+)", output)}
    longest = run = 0
    for seq in range(1, count + 1):
        run = 0 if seq in received else run + 1
        longest = max(longest, run)
    return longest


def test_failure_notification(kirad_small_k, kira_topo, conn_helpers, monkeypatch):
    # the notifications are logged on debug level
    rust_log = os.environ.get("RUST_LOG", "info")
    monkeypatch.setenv("RUST_LOG", f"{rust_log},failure_handling=debug")

    test = KIRATest[str](kira_topo, kirad_binary=kirad_small_k)
    conn_helpers.checkup_timeout(test, timeout=1)
    conn_helpers.retry_sweep(test, conn_helpers.ping_check, test_msg="ping")

    chosen = _choose_failure(test)
    if chosen is None:
        pytest.skip("no path with a link whose failure only the notification reports")
    src, dst, hops, (x, y), encapsulator = chosen
    print(
        f"path {' -> '.join(map(str, hops))}, failing {x} -✗- {y} "
        f"(encapsulated by {encapsulator})"
    )
    link = next(
        link for u, v, link in test.topology.links if {str(u), str(v)} == {x, y}
    )
    nodes = list(test.topology.nodes)
    log_offsets = {n: len(n.logfile.read_text(errors="replace")) for n in nodes}

    ping = subprocess.Popen(
        f"ip netns exec {src.id} ping -6 -O -i {PING_INTERVAL_SECS} -c {PING_COUNT} "
        f"-W 1 {dst.node_id.to_node_ip()}",
        shell=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    time.sleep(PINGS_BEFORE_FAILURE * PING_INTERVAL_SECS)
    link.down()
    output, _ = ping.communicate()
    outage = _longest_outage(output, PING_COUNT)
    print(f"longest outage: {outage} pings of {PING_COUNT}")
    if outage == 0:
        # e.g., the path changed between the traceroute and the failure: no packet
        # needed the notification (a broken notification loses pings)
        pytest.skip(f"no ping was lost after the link {x} - {y} failed")

    # a node reported dropped packets and notified the encapsulating node
    notified = test.topology.nodes.by_tid(encapsulator).node_id
    logs = {n: n.logfile.read_text(errors="replace")[log_offsets[n] :] for n in nodes}
    assert any("Forwarding tier dropped a packet" in log for log in logs.values()), (
        f"no node reported packets dropped after the link {x} - {y} failed"
    )
    assert any(
        f"Notifying {notified} about failed link" in log for log in logs.values()
    ), f"no node notified {encapsulator} ({notified}) about the failed link"

    # the source switched to another path quickly
    assert outage * PING_INTERVAL_SECS <= MAX_OUTAGE_SECS, output
