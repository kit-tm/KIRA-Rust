from pathlib import Path

from .node_config import NodeConfig

REPO_ROOT = Path(__file__).resolve().parents[6]
assert REPO_ROOT.exists(), "unable to locate kira-rust repo"
assert (REPO_ROOT / "Cargo.toml").exists(), "kira-rust repo not detected"

TOPOS_DIR = Path(__file__).resolve().parents[5] / "topos"
assert TOPOS_DIR.exists(), "kira-test topos not found"
