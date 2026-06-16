import ctypes
import ctypes.util
import os
import subprocess
import sys
from pathlib import Path

# exit codes
ERR_CMD_FAILED = 1
ERR_UNSHARE_FAILED = 2
ERR_PERM = 3

IP_NETNS_PINS_DIR = Path("/var/run/netns")


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
    try:
        IP_NETNS_PINS_DIR.mkdir(parents=True, exist_ok=True)
    except PermissionError:
        print(
            "Error: Unable to create mount-point /var/run/netns. ",
            "Try creating manually: mkdir -p /var/run/netns",
        )
        sys.exit(ERR_PERM)

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
    target = IP_NETNS_PINS_DIR.as_posix()
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

    # 4. bring loopback up
    # this is needed for compiling inside unshare isolation
    subprocess.run(["ip", "link", "set", "lo", "up"], check=True)

    return True
