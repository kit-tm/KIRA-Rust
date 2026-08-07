import logging
import os
import shutil
import subprocess
import time
from pathlib import Path

import pytest
from kira_common.paths import REPO_ROOT
from kira_nest.nest.node import KIRANode
from nest.topology import Address

logger = logging.getLogger(__name__)

TOPOS_DIR = REPO_ROOT / "tests" / "topos"
assert TOPOS_DIR.exists(), "kira-test topos not found"

TOPOS = os.environ.get("KIRA_TOPOS")


def built_kirad_binary(
    tmp_path_factory: pytest.TempPathFactory, id: str, features: list[str] | None = None
) -> Path:
    """
    Ensure we have the daemon with the given features built.
    Prefer prebuilt artifact via KIRAD_BIN_<id_suffix>; otherwise build with cargo.
    """

    id_suffix = f"_{id}" if id else ""  # append _ to suffix if set
    daemon_bin = os.environ.get(f"KIRAD_BIN{id_suffix.upper()}")
    if daemon_bin is not None:
        daemon_bin = Path(daemon_bin)
        if daemon_bin.exists():
            return daemon_bin
        else:
            logger.warning(
                f"KIRA_BIN='{daemon_bin}' doesn't exist. Fallback: Building binary."
            )

    # Fallback: build the Rust daemon
    target_kirad = REPO_ROOT / "target" / "debug" / "kirad"
    tmp_bin_dir = tmp_path_factory.mktemp("bin")

    # copy preexisting debug binary to restore it later on
    if target_kirad.exists():
        orig_kirad_dest = tmp_bin_dir / "kirad.orig"
        shutil.copy2(target_kirad, orig_kirad_dest)
        # don't remove binary to avoid builds
    else:
        orig_kirad_dest = None

    # build Rust daemon used in testing
    # even if it exists to avoid outdated binary
    feature_args = ["--features", ",".join(features)] if features else []
    env = os.environ.copy()
    subprocess.run(
        ["cargo", "build", "--locked", "--bin", "kirad"] + feature_args,
        cwd=REPO_ROOT,
        check=True,  # propagate failure
        # preexec_fn=_drop_caps,
        env=env,
    )
    assert target_kirad.exists(), "kirad not build"

    # copy file to tempory destination to avoid overwrites by successive cargo builds
    kirad_dest = tmp_bin_dir / f"kirad{id_suffix}"
    shutil.copy2(target_kirad, kirad_dest)

    # Restore preexisting debug binary
    if orig_kirad_dest is not None:
        shutil.copy2(orig_kirad_dest, target_kirad)

    return kirad_dest


def _topo_files():
    if TOPOS is not None:
        return [Path(file_str).resolve() for file_str in TOPOS.split(os.pathsep)]

    # Fallback: use topo files in "topos" directory

    return sorted(TOPOS_DIR.resolve().glob("*.gml"))


@pytest.fixture(scope="session")
def topos_dir():
    return TOPOS_DIR


@pytest.fixture(scope="session")
def kirad(tmp_path_factory: pytest.TempPathFactory) -> Path:
    return built_kirad_binary(tmp_path_factory, "")


@pytest.fixture(scope="session")
def kirad_small_k(tmp_path_factory: pytest.TempPathFactory) -> Path:
    return built_kirad_binary(tmp_path_factory, "small-k", ["small_buckets"])


@pytest.fixture(scope="session", params=_topo_files(), ids=lambda p: p.name)
def kira_topo(request) -> Path:
    return request.param


@pytest.fixture(autouse=True)
def kira_logs(monkeypatch: pytest.MonkeyPatch, tmp_path: Path):
    monkeypatch.setenv(KIRANode.ENV_LOG_PATH, str(tmp_path / "log"))


class ConnectivityHelpers:
    def __init__(self, subtests):
        self.subtests = subtests

    def retry_sweep(
        self,
        test,
        check_func,
        test_msg="check",
        fail_msg=None,
        max_attempts=3,
        verbose_traceroute_on_fail=True,
        cool_down=1.0,
        respect_connected_components=True,
        gate=True,
    ):
        """
        Sweeps through all node pairs, retrying failing checks across multiple passes.
        """
        if fail_msg is None:
            fail_msg = test_msg.capitalize()

        topology = test.topology
        node_list = list(topology.nodes)
        # Only apply check function to two nodes in the same connected component
        if respect_connected_components:
            conn = dict(topology.connected_components())
            pairs = [
                (src, dst)
                for src in node_list
                for dst in node_list
                if conn[src] == conn[dst]
            ]
        else:
            pairs = [(src, dst) for src in node_list for dst in node_list]
        pending = list(pairs)

        for attempt in range(max_attempts):
            logger.debug(
                f"\n--- {test_msg.upper()} "
                f"Sweep Attempt {attempt + 1}/{max_attempts} ---"
            )
            next_pending = []
            for src, dst in pending:
                if check_func(src, dst):
                    logger.debug(
                        f"  [SUCCESS] {src} -> {dst} passed on attempt {attempt + 1}",
                    )
                    continue
                logger.debug(f"  [FAILED]  {src} -> {dst} failed, will retry")
                next_pending.append((src, dst))

            pending = next_pending
            if not pending:
                logger.debug(
                    f"  [INFO] All checks passed early on attempt {attempt + 1}!",
                )
                break
            if attempt < max_attempts - 1:
                time.sleep(cool_down)

        logger.debug(f"\n--- Finalizing {test_msg} Subtest Reports ---")
        for src, dst in pairs:
            with self.subtests.test(msg=test_msg, src=str(src), dst=str(dst)):
                success = (src, dst) not in pending
                if verbose_traceroute_on_fail and not success:
                    test.traceroute(src, dst, verbose=True)
                assert success, (
                    f"{fail_msg} {src} -> {dst} failed after {max_attempts} attempts"
                )

        if gate:
            assert not pending, "Gated retry sweep should not fail on subtests"

        return pending

    def checkup_timeout(self, test, timeout=1, gate=True):
        failed = False
        up_timeout = time.time() + timeout
        for n in test.topology.nodes:
            with self.subtests.test(msg="checkup", n=str(n)):
                while not n.is_up():
                    if time.time() > up_timeout:
                        failed = True
                        break
                    time.sleep(0.05)
                assert n.is_up(), f"Node {n} failed to come up within {timeout} second"

        if gate:
            assert not failed, "All nodes have to be up"

    @staticmethod
    def ping_check(src, dst):
        if src == dst:
            return True
        ip_dst = Address(str(dst.node_id.to_node_ip()))
        return src.ping(ip_dst, packets=1, verbose=0, timeout=1)

    @staticmethod
    def traceroute_check(test, src, dst):
        if src == dst:
            return True
        return test.traceroute(src, dst, verbose=False)


@pytest.fixture
def conn_helpers(subtests):
    return ConnectivityHelpers(subtests)
