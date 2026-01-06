import logging
import os
import shutil
import subprocess
from pathlib import Path

import prctl
import pytest
from kira_common import REPO_ROOT, TOPOS_DIR
from kira_nest.nest.node import KIRANode

logger = logging.Logger(__name__)

TOPOS = os.environ.get("KIRA_TOPOS")


def _drop_caps():
    prctl.cap_permitted.limit()


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
