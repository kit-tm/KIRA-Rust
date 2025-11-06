from dataclasses import dataclass


@dataclass
class NodeConfig:
    node_id: str
    ipv6: str
    name: str
    image: str
    otel: bool = False
