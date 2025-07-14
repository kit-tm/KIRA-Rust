import argparse
import matplotlib.pyplot as plt
import networkx as nx
import sys


def main(args):
    G = nx.readwrite.read_gml(args.test_gml)

    pos = nx.kamada_kawai_layout(G)
    nx.draw_networkx(G, pos=pos, font_color="w")
    plt.savefig(sys.stdout.buffer, transparent=True, dpi=200)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Show Grpah of gml")
    parser.add_argument("test_gml", type=str, help="The gml file")
    args = parser.parse_args()
    main(args)
