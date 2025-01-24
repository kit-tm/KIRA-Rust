import argparse
import matplotlib
import matplotlib.pyplot as plt
import networkx as nx

def main(args):
    G=nx.readwrite.read_gml(args.test_gml)
    nx.draw_networkx(G,node_size=500,font_color='w')
    plt.show()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description='Show Grpah of gml')
    parser.add_argument('test_gml', type=str, help="The gml file")
    args = parser.parse_args()
    main(args)
