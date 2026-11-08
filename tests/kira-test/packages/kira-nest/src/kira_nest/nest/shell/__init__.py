import argparse
import logging
import os
import random
import subprocess
import sys
from pathlib import Path

import networkx as nx
from kira_common.domain import (
    ENV_NODE_IP_PREFIX,
    ENV_PATH_IP_PREFIX,
    IPPrefixes,
    parse_ip_prefix,
)
from kira_common.paths import REPO_ROOT

from kira_nest.unshare import probe_netns_cap, unshare_emulation

# exit codes
ERR_CMD_FAILED = 1
ERR_UNSHARE_FAILED = 2
ERR_PERM = 3


logger = logging.getLogger(__name__)
logger.setLevel(logging.INFO)


def build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Nest Test Script")
    parser.add_argument(
        "test_gml",
        type=str,
        nargs="?",
        default="-",
        help="The gml file path or '-' for stdin",
    )
    parser.add_argument(
        "--otel",
        action="store_true",
        help="Enable open telemetry exports on all nodes",
    )
    parser.add_argument(
        "-q",
        "--quiet",
        action="store_true",
        help="Makes commands less verbose",
    )
    parser.add_argument(
        "-b",
        "--binary",
        default=REPO_ROOT / "target" / "debug" / "kirad",
        type=Path,
        help="path to kirad binary",
    )
    parser.add_argument(
        "-l",
        "--log-processing",
        action="store_true",
        help=(
            "Enable post processing of log files, substituting node-ids with node tids"
        ),
    )
    parser.add_argument(
        "--flamegraph",
        nargs="*",
        help="Create flamegraph for node with tid",
    )
    parser.add_argument(
        "filename",
        nargs="?",
        help="Commands to execute non-interactively",
        type=argparse.FileType("r"),
    )
    parser.add_argument(
        "--seed", type=int, default=None, help="Random seed for reproducibility"
    )
    default_prefixes = IPPrefixes()
    parser.add_argument(
        "--node-ip-prefix",
        type=parse_ip_prefix,
        default=None,
        help=(
            "/16 IPv6 prefix of NodeIPs passed to all nodes"
            f" (default: ${ENV_NODE_IP_PREFIX} or {default_prefixes.node_ip})"
        ),
    )
    parser.add_argument(
        "--path-ip-prefix",
        type=parse_ip_prefix,
        default=None,
        help=(
            "/16 IPv6 prefix of PathIPs passed to all nodes"
            f" (default: ${ENV_PATH_IP_PREFIX} or {default_prefixes.path_ip})"
        ),
    )
    return parser


def ip_prefixes_from_args(
    parser: argparse.ArgumentParser, args: argparse.Namespace
) -> IPPrefixes:
    """Prefixes from the arguments, falling back to the environment variables."""
    default_prefixes = IPPrefixes()
    try:
        node_ip = args.node_ip_prefix or parse_ip_prefix(
            os.environ.get(ENV_NODE_IP_PREFIX, str(default_prefixes.node_ip))
        )
        path_ip = args.path_ip_prefix or parse_ip_prefix(
            os.environ.get(ENV_PATH_IP_PREFIX, str(default_prefixes.path_ip))
        )
        return IPPrefixes(node_ip, path_ip)
    except ValueError as e:
        parser.error(str(e))


def run_shell() -> None:  # noqa: PLR0912
    parser = build_arg_parser()
    args = parser.parse_args()
    ip_prefixes = ip_prefixes_from_args(parser, args)
    if args.seed:
        random.seed(args.seed)

    # Load the configuration from the GML file
    graph: nx.Graph
    if args.test_gml == "-":
        graph = nx.readwrite.read_gml(sys.stdin.buffer)
    else:
        graph = nx.readwrite.read_gml(args.test_gml)

    if not args.otel:
        # run emulation completely isolated from host Linux system
        # this allows us to run NeST without CAP_SYS_ADMIN (needed for netns creation)
        unshared = False if probe_netns_cap() else unshare_emulation()
    else:
        # not possible for open telemetry (unless the server is run inside too)
        # because you'd need veths (requires CAP_NET_ADMIN) or pasta (https://passt.top/passt/about/).
        unshared = False

        # enable OTel for all nodes
        for _, cfg in graph.nodes(data="config"):
            cfg["otel"] = True

    # import lazily to delay privilege checks of NeST after potential unshare
    from kira_nest.nest.shell.debug_shell import DebugShell  # noqa: PLC0415
    from kira_nest.nest.test import KIRATest  # noqa: PLC0415

    # Create and run the test

    test = KIRATest[str](
        graph,
        kirad_binary=args.binary,
        perf=args.flamegraph,
        ip_prefixes=ip_prefixes,
    )
    shell = DebugShell(
        test, unshared=unshared, post_process_log_files=args.log_processing
    )
    shell.quiet = args.quiet

    if args.filename is not None:
        # non-interactive
        shell.exit_on_failure = True
        shell.print_cmd = True

        for cmd_line in args.filename.read().splitlines():
            print(cmd_line)
            shell.onecmd(cmd_line)
            if shell.failure:
                print("Last command failed. Exiting...")
                break
            print()
        shell.do_exit("")
    elif args.test_gml == "-":
        # Ensure interactive repl still works although stdin was used to pipe
        try:
            with open("/dev/tty") as tty:
                sys.stdin = tty
                shell.cmdloop()
        finally:
            # restore to default
            sys.stdin = sys.__stdin__
    else:
        shell.cmdloop()

    # Directly generating the svg with flamegraph
    # doesn't work because of the ungraceful stop by NeST
    # => perf then generate flamegraph
    if args.flamegraph:
        logger.info("Generating flamegraphs")
        for tid in args.flamegraph:
            node = test.topology.nodes[tid]
            subprocess.run(
                [
                    "flamegraph",
                    "-o",
                    f"log/flamegraph-{node}.svg",
                    "--perfdata",
                    f"log/perf-{node}.data",
                    "--subtitle",
                    "kirad@k18",
                ],
                check=False,
            )

    exit_code = ERR_CMD_FAILED if shell.failure else 0
    sys.exit(exit_code)


if __name__ == "__main__":
    run_shell()
