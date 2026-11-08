from ipaddress import IPv6Address, IPv6Network

import pytest
from kira_common import domain
from kira_common.domain import (
    IPPrefixes,
    NodeID,
    NodeIP,
    Path,
    PathID,
    PathIP,
    get_ip_prefixes,
    parse_ip_prefix,
    set_ip_prefixes,
)
from kira_common.node_config import NodeConfig

NODE_ID = NodeID.fromhex("0123456789abcdef0123456789ab")
PATH_ID = PathID.fromhex("00112233445566778899aabbccdd")
CUSTOM = IPPrefixes(IPv6Network("fd00::/16"), IPv6Network("fdaa::/16"))


@pytest.fixture(autouse=True)
def restore_prefixes():
    original = get_ip_prefixes()
    yield
    set_ip_prefixes(original)


def test_parse_ip_prefix():
    assert parse_ip_prefix("fd00::/16") == IPv6Network("fd00::/16")
    assert parse_ip_prefix("fd00::") == IPv6Network("fd00::/16")
    with pytest.raises(ValueError):
        parse_ip_prefix("fd00::/48")
    with pytest.raises(ValueError):
        parse_ip_prefix("fd00::1/16")


def test_prefixes_must_differ():
    with pytest.raises(ValueError):
        IPPrefixes("fd00::/16", "fd00::/16")


def test_prefixes_from_env(monkeypatch):
    monkeypatch.setenv(domain.ENV_NODE_IP_PREFIX, "fd00::/16")
    monkeypatch.delenv(domain.ENV_PATH_IP_PREFIX, raising=False)
    prefixes = IPPrefixes.from_env()
    assert prefixes.node_ip == IPv6Network("fd00::/16")
    assert prefixes.path_ip == domain.DEFAULT_PATH_IP_PREFIX


def test_default_addresses():
    set_ip_prefixes(IPPrefixes())
    assert NODE_ID.to_node_ip() == IPv6Address(
        "fc00:0123:4567:89ab:cdef:0123:4567:89ab"
    )
    assert PATH_ID.to_path_ip() == IPv6Address(
        "fcaa:0011:2233:4455:6677:8899:aabb:ccdd"
    )


def test_addresses_use_configured_prefixes():
    set_ip_prefixes(CUSTOM)
    assert NODE_ID.to_node_ip() == IPv6Address(
        "fd00:0123:4567:89ab:cdef:0123:4567:89ab"
    )
    assert PATH_ID.to_path_ip() == IPv6Address(
        "fdaa:0011:2233:4455:6677:8899:aabb:ccdd"
    )
    assert Path([NODE_ID]).to_path_ip() in CUSTOM.path_ip
    # addresses with the old prefixes are no KIRA addresses anymore
    with pytest.raises(ValueError):
        NodeIP("fc00:0123:4567:89ab:cdef:0123:4567:89ab")
    with pytest.raises(ValueError):
        PathIP("fcaa:0011:2233:4455:6677:8899:aabb:ccdd")


def test_kirad_args():
    assert CUSTOM.kirad_args() == [
        "--node-ip-prefix",
        "fd00::/16",
        "--path-ip-prefix",
        "fdaa::/16",
    ]
    assert IPPrefixes().is_default()
    assert not CUSTOM.is_default()


def test_node_config_accepts_ipv6_with_other_prefix():
    # topology files store the NodeIP derived with the default prefix
    set_ip_prefixes(CUSTOM)
    cfg = NodeConfig(
        node_id=NODE_ID.hex(), ipv6="fc00:0123:4567:89ab:cdef:0123:4567:89ab"
    )
    assert cfg.ipv6 == IPv6Address("fd00:0123:4567:89ab:cdef:0123:4567:89ab")
    with pytest.raises(AssertionError):
        NodeConfig(node_id=NODE_ID.hex(), ipv6="fc00::1")
