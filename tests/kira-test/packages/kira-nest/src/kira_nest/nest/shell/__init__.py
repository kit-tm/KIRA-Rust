import argparse
import sys
from pathlib import Path

import networkx as nx
from kira_common.paths import REPO_ROOT

from kira_nest.unshare import probe_netns_cap, unshare_emulation

# exit codes
ERR_CMD_FAILED = 1
ERR_UNSHARE_FAILED = 2
ERR_PERM = 3


def build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Nest Test Script")
    parser.add_argument("test_gml", type=str, help="The gml file")
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
        "filename",
        nargs="?",
        help="Commands to execute non-interactively",
        type=argparse.FileType("r"),
    )
    return parser


def run_shell() -> None:
    parser = build_arg_parser()
    args = parser.parse_args()

    # Load the configuration from the GML file
    graph: nx.Graph = nx.readwrite.read_gml(args.test_gml)
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

    test = KIRATest[str](graph, kirad_binary=args.binary)
    shell = DebugShell(test, unshared)
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
    else:
        # interactive
        shell.cmdloop()

    exit_code = ERR_CMD_FAILED if shell.failure else 0
    sys.exit(exit_code)


if __name__ == "__main__":
    run_shell()
