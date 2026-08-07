import argparse
import random
import sys
from collections.abc import Generator

import networkx as nx

from kira_gml import eprint


def generate_graph(n: int, args: argparse.Namespace) -> nx.Graph:
    model = args.model.lower()
    if model == "erdos":
        p = args.p
        eprint(f"Generating Erdos-Renyi graph (n={n}, p={p})...")
        return nx.erdos_renyi_graph(n, p)

    elif model == "barabasi":
        m = args.m
        eprint(f"Generating Barabasi-Albert graph (n={n}, m={m})...")
        return nx.barabasi_albert_graph(n, m)

    elif model == "watts":
        k = args.k
        p = args.p
        eprint(f"Generating Watts-Strogatz graph (n={n}, k={k}, p={p})...")
        return nx.watts_strogatz_graph(n, k, p)

    else:
        raise ValueError(
            f"Unknown graph model: {model}. Choose from 'erdos', 'barabasi', 'watts'."
        )


def generate_graphs(args: argparse.Namespace) -> Generator[nx.Graph]:
    n = args.nodes
    c = args.components

    # distributed node count randomly among components
    cuts = sorted(random.sample(range(n), c - 1))  # cut points
    # difference between consecutive cuts
    distribution = [a - b for a, b in zip([*cuts, n], [0, *cuts], strict=True)]

    yield from (generate_graph(n, args) for n in distribution)


def build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Generate random NetworkX graphs from command-line arguments."
    )

    # Core arguments
    parser.add_argument(
        "model",
        type=str,
        choices=["erdos", "barabasi", "watts"],
        help="Random graph model to use",
    )
    parser.add_argument(
        "-n",
        "--nodes",
        type=int,
        default=50,
        help="Number of nodes in the graph",
    )
    parser.add_argument(
        "--seed", type=int, default=None, help="Random seed for reproducibility"
    )

    # Model-specific parameters
    parser.add_argument(
        "--p",
        type=float,
        default=0.1,
        help=(
            "Edge probability for Erdos-Renyi model; "
            "Rewiring probability for Watts-Strogatz model"
        ),
    )
    parser.add_argument(
        "-m",
        type=int,
        default=2,
        help="Number of edges to attach for Barabasi-Albert model",
    )
    parser.add_argument(
        "-k",
        type=int,
        default=4,
        help="Number of nearest neighbors for Watts-Strogatz model",
    )
    parser.add_argument(
        "-c",
        "--components",
        type=int,
        default=1,
        help="Lower bound of separate components to generate (all same graph model)",
    )

    return parser


def generate_random_gml() -> None:
    parser = build_arg_parser()
    args = parser.parse_args()
    if args.seed:
        random.seed(args.seed)

    graphs = generate_graphs(args)
    graph = nx.disjoint_union_all(graphs)
    nx.write_gml(graph, sys.stdout.buffer)

    # Print quick summary statistics
    eprint("Graph generated successfully!")
    eprint(f" - Nodes: {graph.number_of_nodes()}")
    eprint(f" - Edges: {graph.number_of_edges()}")
    is_connected = nx.is_connected(graph)
    eprint(f" - Is connected: {is_connected}")
    for i, comp in enumerate(nx.connected_components(graph)):
        eprint(f" - Component {i}: {sorted(comp)}")


if __name__ == "__main__":
    generate_random_gml()
