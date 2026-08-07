import argparse
import sys

import matplotlib.pyplot as plt
import networkx as nx


def build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Draw Graph (stdout)")
    parser.add_argument(
        "test_gml",
        type=str,
        nargs="?",
        default="-",
        help="The gml file path or '-' for stdin",
    )
    return parser


def show_graph() -> None:
    parser = build_arg_parser()
    args = parser.parse_args()

    gml_src = sys.stdin.buffer if args.test_gml == "-" else args.test_gml
    graph = nx.readwrite.read_gml(gml_src)

    pos = nx.kamada_kawai_layout(graph)
    nx.draw_networkx(graph, pos=pos, font_color="w")
    plt.savefig(sys.stdout.buffer, format="png", transparent=True, dpi=200)


if __name__ == "__main__":
    show_graph()
