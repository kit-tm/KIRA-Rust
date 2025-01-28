from kira_net import KIRANetwork
try:
    containernet = True
    from kira_net import KIRAContainernetNetwork
    from mininet.net import Containernet
    from mininet.node import Controller
except ImportError:
    containernet = False


import docker
import networkx as nx

import sys
import argparse

import logging
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

if __name__ == '__main__':
    operations = [
        "create", "connect", "start", "stop", "prune",
        "store", "fetch",
        "logs", "node-id",
        "pn-table", "routing-table", "vicinity-graph", "local-hashtable",
        "draw",
        "test-connectivity"
    ]

    parser = argparse.ArgumentParser(description='kira topology generator')
    parser.add_argument('operation', type=str, choices=operations)
    parser.add_argument('-n', '--node-id', type=int, required=False, dest="node",
                        help="ID of the node to which to send store/fetch requests")
    parser.add_argument('--key', type=str, required=False, dest="key",
                        help="Reference used by the store/fetch operation")
    parser.add_argument('-d', '--data', type=str, required=False, dest="data",
                        help="Data to store at given node")
    parser.add_argument('--edges', type=argparse.FileType("r"), required=True,
                        help="Path pointing to an edgelist")
    parser.add_argument('--img', type=str,
                        help="Docker image used by the node containers")
    parser.add_argument('--seed', type=str, required=False,
                        help="Seed used to generate Node-IDs")
    parser.add_argument('--plain', required=False, dest="plain",
                        action='store_true',
                        help="Don't replace NIDs with topology IDs in outputs")
    if containernet:
        parser.add_argument('--containernet', required=False, dest="backend",
                            action='store_const', const="containernet",
                            help="Use Containernet as backend")
    parser.add_argument('--docker', required=False, dest="backend",
                        action='store_const', const="docker",
                        help="Use Docker as backend")
    parser.add_argument('--yes', required=False, dest="yes",
                        action='store_true',
                        help="Answer all questions with yes")
    args = parser.parse_args()

    edgefile = args.edges
    try:
        graph = nx.read_edgelist(edgefile)
    except FileNotFoundError:
        logger.error(f"Couldn't read edgelist file: {edgefile}!")
        sys.exit(202)

    result = None
    try:
        if args.backend == "containernet":
            from mininet.net import Containernet
            from mininet.node import Controller

            net = Containernet(controller=Controller)
            network = KIRAContainernetNetwork(graph, net, seed=args.seed)
        else:
            network = KIRANetwork(graph, seed=args.seed)

        operation = args.operation.lower()
        if operation == "create":
            if args.img is None:
                logger.error("No default image to use was specified!")
                exit(101)
            network.create(args.img)
            logger.info("All nodes are created and ready to be started.")
        elif operation == "start":
            if args.backend == "containernet":
                network.create(args.img)
                network.connect()
            network.start()
            logger.info("All nodes are started.")
        elif operation == "stop":
            network.stop()
            logger.info("All nodes are stopped.")
        elif operation == "connect":
            network.connect()
            logger.info("All edges are connected.")
        elif operation == "prune":
            if not args.yes:
                answer = input(
                    "Pruning might delete other containers not managed by this script. Do you really want to continue? y/N:  ")
                if not answer.strip().lower() == "y":
                    logger.info("Stopped pruning.")
                    sys.exit()
            network.prune()
            logger.info("Pruning done.")
        elif operation == "store":
            result = network.get_node(args.node).store(args.key, args.data)
        elif operation == "fetch":
            result = network.get_node(args.node).fetch(args.key)
        elif operation == "node-id":
            res = network.get_node(args.node).nid
            print(res.hex())
            grouped = res.hex(':', 2)
            print(grouped)
        elif operation == "test-connectivity":
            node = args.node
            if node is None:
                all_connected = network.test_connectivity()
            else:
                all_connected = network.test_connectivity(origin=node)
            if not all_connected:
                sys.exit(4)
        elif operation == "logs":
            result = network.get_node(args.node).logs()
        elif operation == "pn-table":
            result = network.get_node(args.node).pn_table()
        elif operation == "routing-table":
            result = network.get_node(args.node).routing_table()
        elif operation == "vicinity-graph":
            result = network.get_node(args.node).vicinity_graph()
        elif operation == "local-hashtable":
            result = network.get_node(args.node).local_hashtable()
        elif operation == "draw":
            import matplotlib.pyplot as plt

            pos = nx.kamada_kawai_layout(graph)
            nx.draw_networkx(graph, pos)
            plt.savefig(sys.stdout.buffer, transparent=True, dpi=200)
        else:
            logger.warn(f"Unknown operation: {operation}")
            sys.exit(2)

    except docker.errors.APIError as err:
        logger.exception(err)
        sys.exit(1)

    if result is None:
        sys.exit(0)

    if args.plain:
        print(result)
        sys.exit(0)

    # process output
    for n in graph:
        nid = network.get_node(n).nid
        if nid is None:
            continue

        nid = nid.hex().upper()
        result = result.replace(nid, n)

    print("PROCESSED OUTPUT:")
    print(result)
