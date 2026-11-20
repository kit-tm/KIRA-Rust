"""Aggregation of the R²/KAD message statistics written by kirad (`--benchmark-path`).

Usage: `python -m kira_nest.stats <dir or files...> [--json]`

Sums the per-node JSON files to network-wide totals per message kind and prints them
together with the average per node and second.
"""

import argparse
import json
import pathlib
import sys
from collections import defaultdict
from typing import Any

COUNTERS = (
    "tx_msgs",
    "tx_payload_bytes",
    "tx_wire_bytes",
    "rx_msgs",
    "rx_payload_bytes",
    "rx_wire_bytes",
)


def load(paths: list[pathlib.Path]) -> list[dict[str, Any]]:
    files: list[pathlib.Path] = []
    for path in paths:
        files.extend(sorted(path.glob("*.json")) if path.is_dir() else [path])
    return [json.loads(f.read_text()) for f in files]


def aggregate(nodes: list[dict[str, Any]]) -> dict[str, Any]:
    """Sums the counters of all nodes; rates are per node, averaged over all nodes."""
    per_kind: dict[str, dict[str, float]] = defaultdict(
        lambda: dict.fromkeys(COUNTERS, 0)
    )
    total = dict.fromkeys(COUNTERS, 0)
    node_seconds = sum(n["duration_s"] for n in nodes)
    for node in nodes:
        for kind, counters in node["per_kind"].items():
            for c in COUNTERS:
                per_kind[kind][c] += counters[c]
        for c in COUNTERS:
            total[c] += node["total"][c]

    def with_rates(counters: dict[str, float]) -> dict[str, float]:
        rates = {
            f"{c}_per_node_s": counters[c] / node_seconds if node_seconds else 0.0
            for c in COUNTERS
        }
        return counters | rates

    return {
        "nodes": len(nodes),
        "mean_duration_s": node_seconds / len(nodes) if nodes else 0.0,
        "total": with_rates(total),
        "per_kind": {k: with_rates(v) for k, v in sorted(per_kind.items())},
        "rx_invalid_msgs": sum(n["rx_invalid_msgs"] for n in nodes),
        "tx_errors": sum(n["tx_errors"] for n in nodes),
    }


def print_table(agg: dict[str, Any]) -> None:
    print(f"{agg['nodes']} nodes, mean duration {agg['mean_duration_s']:.1f} s")
    print(
        f"{'kind':<16} {'tx msgs':>10} {'tx bytes':>12} {'msgs/node/s':>12} "
        f"{'B/node/s':>10} {'rx msgs':>10} {'rx bytes':>12}"
    )
    rows = list(agg["per_kind"].items()) + [("TOTAL", agg["total"])]
    for kind, c in rows:
        print(
            f"{kind:<16} {c['tx_msgs']:>10} {c['tx_wire_bytes']:>12} "
            f"{c['tx_msgs_per_node_s']:>12.2f} {c['tx_wire_bytes_per_node_s']:>10.1f} "
            f"{c['rx_msgs']:>10} {c['rx_wire_bytes']:>12}"
        )
    print("(bytes on the wire incl. IPv6/UDP headers; rates: sent per node and second)")
    if agg["rx_invalid_msgs"] or agg["tx_errors"]:
        print(
            f"invalid received: {agg['rx_invalid_msgs']}, "
            f"send errors: {agg['tx_errors']}"
        )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("paths", nargs="+", type=pathlib.Path)
    parser.add_argument(
        "--json", action="store_true", help="print the aggregate as JSON"
    )
    args = parser.parse_args()
    nodes = load(args.paths)
    if not nodes:
        sys.exit("no statistics files found")
    agg = aggregate(nodes)
    if args.json:
        print(json.dumps(agg, indent=2))
    else:
        print_table(agg)


if __name__ == "__main__":
    main()
