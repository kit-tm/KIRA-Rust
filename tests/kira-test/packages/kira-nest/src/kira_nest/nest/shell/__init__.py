import argparse
import ctypes
import ctypes.util
import os
import sys
from pathlib import Path

import networkx as nx
from kira_common.paths import REPO_ROOT

# exit codes
ERR_CMD_FAILED = 1
ERR_UNSHARE_FAILED = 2
ERR_PERM = 3


def probe_netns_cap() -> bool:
    # Currently only works if root:

    pid = os.fork()

    if pid == 0:
        # child
        try:
            os.unshare(os.CLONE_NEWNET)
            os._exit(0)
        except PermissionError:
            os._exit(ERR_PERM)
    else:
        # parent
        _, status = os.waitpid(pid, 0)
        exit_code = os.WEXITSTATUS(status)

        if exit_code == 0:
            return True
        elif exit_code == ERR_PERM:
            return False
        else:
            raise Exception(
                f"Unknown Exit Code observed on unshare(CLONE_NEWNET): {exit_code}"
            )


def unshare_emulation() -> bool:
    # man unshare 2
    real_uid = os.getuid()
    real_gid = os.getgid()

    # 1. Create dedicated namespace for emulation
    try:
        os.unshare(os.CLONE_NEWUSER | os.CLONE_NEWNET | os.CLONE_NEWNS)
    except PermissionError:
        print("Error: Ensure unprivileged user namespaces are enabled on your system.")
        return False

    # 2. Map root to real user
    try:
        with open("/proc/self/uid_map", "w") as f:
            f.write(f"0 {real_uid} 1")

        # required if mapping gid
        with open("/proc/self/setgroups", "w") as f:
            f.write("deny")
        with open("/proc/self/gid_map", "w") as f:
            f.write(f"0 {real_gid} 1")

    except PermissionError:
        print("Error: Unable to map uid and gid")
        sys.exit(ERR_UNSHARE_FAILED)

    # 3. mount -t tmpfs tmpfs /var/run/netns
    # otherwise network namespaces can't be created using ip netns add

    # create mount-point
    IP_NETNS_PINS = Path("/var/run/netns")
    try:
        IP_NETNS_PINS.mkdir(parents=True, exist_ok=True)
    except PermissionError as e:
        print(
            "Error: Unable to create mount-point /var/run/netns. Try creating manually: mkdir -p /var/run/netns"
        )

    libc = ctypes.CDLL(ctypes.util.find_library("c"), use_errno=True)
    libc.mount.argtypes = (
        ctypes.c_char_p,
        ctypes.c_char_p,
        ctypes.c_char_p,
        ctypes.c_ulong,
        ctypes.c_void_p,
    )
    # int mount(const char *source, const char *target,
    #           const char *filesystemtype, unsigned long mountflags,
    #           const void *_Nullable data);
    source = "tmpfs"
    target = IP_NETNS_PINS.as_posix()
    filesystemtype = "tmpfs"
    mountflags = 0
    data = None
    mount_res = libc.mount(
        source.encode(),
        target.encode(),
        filesystemtype.encode(),
        mountflags,
        data,
    )

    if mount_res != 0:
        errno = ctypes.get_errno()
        raise OSError(errno, f"Mounting tmpfs on {target} failed: {os.strerror(errno)}")

    print(
        "The emulation is run in complete isolation from the host system.",
        "For further information read unshare(2).",
    )
    return True


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
