import importlib.resources
from pathlib import Path

REPO_ROOT = Path.cwd()

# detect REPO_ROOT if in workspace
_current_dir = Path(__file__).resolve().parent
_repo_root_project = _current_dir.parents[5]
_cargo_toml = _repo_root_project / "Cargo.toml"
if _repo_root_project.exists() and _cargo_toml.exists():
    REPO_ROOT = _repo_root_project


NFTABLES_CONF = importlib.resources.files(__package__).joinpath("nftables.conf")
assert NFTABLES_CONF.is_file(), "nftables.conf not found"
