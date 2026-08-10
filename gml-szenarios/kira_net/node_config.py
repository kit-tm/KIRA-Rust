from dataclasses import dataclass, field, InitVar, asdict
from typing import Optional, Dict, List
from random import Random

from ipaddress import IPv4Address
from networkx import Graph


@dataclass
class DefaultNodeConfig(object):
    name: str = "default"
    docker_image: str = "kira"
    api_port: int = 8080
    dcmd: str = "/usr/bin/supervisord -c /etc/supervisord.conf"
    dns: Optional[List[str]] = None

    sysctls: Dict[str, int] = field(default_factory=lambda: dict())
    environments: Dict[str, str] = field(default_factory=lambda: dict())

    seed: InitVar[int] = "1234"
    debug_level: InitVar[str] = "debug"

    def __post_init__(self, seed, debug_level):
        self._non_default_values = (
            (k, v)
            for (k, v) in {**asdict(self), "debug_level": debug_level}.items()
            if v is not None
        )

        self.sysctls.setdefault("net.ipv6.conf.default.disable_ipv6", 0)
        # disable host-interface
        # self.sysctls.setdefault('net.ipv6.conf.eth0.disable_ipv6', 1)
        self.sysctls.setdefault("net.ipv6.conf.all.forwarding", 1)

        self.environments.setdefault("RUST_LOG", debug_level)
        self.rng = Random(seed)

    def save_in_graph(self, G: Graph):
        if "defaults" not in G.graph:
            G.graph["defaults"] = dict()

        G.graph["defaults"][self.name] = dict(self._non_default_values)

    @classmethod
    def from_graph(cls, G: Graph, name: str):
        return cls(**G.graph["defaults"][name])


@dataclass
class NodeConfig(object):
    name: str = None
    node_id: Optional[bytes] = None
    docker_image: Optional[str] = None
    api_port: Optional[int] = None
    dcmd: Optional[str] = None
    dns: Optional[List[str]] = None

    sysctls: Optional[Dict[str, int]] = None
    environments: Optional[Dict[str, str]] = None

    ip_v4: InitVar[IPv4Address] = None
    debug_level: InitVar[Optional[str]] = None
    default: InitVar[Optional[DefaultNodeConfig]] = None

    def __post_init__(self, ip_v4, debug_level, default):
        self._non_default_values = (
            (k, v)
            for (k, v) in {
                **asdict(self),
                "ip_v4": ip_v4,
                "debug_level": debug_level,
                "default": default.name,
            }.items()
            if v is not None
        )

        if default is None:
            default = DefaultNodeConfig()

        if self.node_id is None:
            self.node_id = default.rng.getrandbits(14 * 8).to_bytes(14, "big")

        if self.docker_image is None:
            self.docker_image = default.docker_image

        if self.api_port is None:
            self.api_port = default.api_port

        if self.dcmd is None:
            self.dcmd = default.dcmd

        if self.dns is None:
            self.dns = default.dns

        if self.sysctls is None:
            self.sysctls = default.sysctls
        else:
            self.sysctls = {**default.sysctls, **self.sysctls}

        if self.environments is None:
            self.environments = default.environments
        else:
            self.environments = {**default.environments, **self.environments}

        if ip_v4 is not None:
            self.environments["KIRA_IPV4"] = ip_v4

        if debug_level is not None:
            self.environments["RUST_LOG"] = debug_level

    def save_in_graph(self, G: Graph, label):
        # TODO: ensure defaults are saved prior
        for k, v in self._non_default_values:
            G.nodes[label][k] = v

    @classmethod
    def from_graph(cls, G: Graph, label):
        default_name = G.nodes[label]["default"]
        default_config = DefaultNodeConfig.from_graph(G, default_name)

        return cls(**{**G.nodes[label], "default": default_config})
