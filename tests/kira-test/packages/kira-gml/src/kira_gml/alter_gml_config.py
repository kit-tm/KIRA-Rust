#!/bin/env python
import argparse
import random
import sys

import networkx as nx
from kira_common import NodeConfig
from kira_common.domain import NodeID

from kira_gml import eprint


def build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Alter the node config of KIRA topology files in GML format"
    )

    parser.add_argument(
        "test_gml",
        type=str,
        nargs="?",
        default="-",
        help="The gml file path or '-' for stdin",
    )
    parser.add_argument(
        "--seed", type=int, default=None, help="Random seed for reproducibility"
    )

    transformations = parser.add_subparsers(
        dest="transformation",
        required=True,
        help="Transformation of the NodeConfigs",
    )
    randomize = transformations.add_parser("randomize", help="Randomize NodeIDs")
    randomize.add_argument(
        "nodes",
        nargs="*",
        type=str,
        default=[],
        help="Topology IDS of the nodes to randomize (leave empty for all nodes)",
    )

    return parser


def handle_randomize(args: argparse.Namespace, graph: nx.Graph) -> None:
    nodes = args.nodes if args.nodes else graph.nodes
    for tid, cfg in graph.nodes(data="config"):
        assert type(cfg) is NodeConfig
        if tid in nodes:
            eprint(f"Randomizing NodeID of '{tid}' ...")
            cfg.node_id = NodeID.random()


def alter_gml_config() -> None:
    parser = build_arg_parser()
    args = parser.parse_args()
    if args.seed:
        random.seed(args.seed)

    # 1. Load GML
    gml_src = sys.stdin.buffer if args.test_gml == "-" else args.test_gml
    graph = nx.readwrite.read_gml(gml_src)

    # 2. Convert Config from dict
    for tid, raw_cfg in graph.nodes(data="config", default={}):
        cfg = NodeConfig(**raw_cfg)
        graph.nodes[tid]["config"] = cfg

    # 3. Apply transformation(s)
    eprint("Applying transformations ...")
    if args.transformation == "randomize":
        handle_randomize(args, graph)
    else:
        raise NotImplementedError(f"Unknown transformation: {args.transformation}")

    # 4. Convert NodeConfig to dict
    for tid, cfg in graph.nodes(data="config"):
        assert type(cfg) is NodeConfig

        graph.nodes[tid]["config"] = cfg.asdict()

    eprint()

    # 5. Write to gml
    nx.write_gml(graph, sys.stdout.buffer)


if __name__ == "__main__":
    alter_gml_config()
