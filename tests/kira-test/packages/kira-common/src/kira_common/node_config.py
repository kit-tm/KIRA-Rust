from kira_common.domain import NodeID, NodeIP


class NodeConfig:
    _name_counter = 0

    def __init__(
        self,
        node_id: str | None = None,
        ipv6: str | None = None,
        name: str | None = None,
        image: str = "kirad",
        otel: bool = False,
    ):
        if node_id is None:
            self.node_id = NodeID.random()
        else:
            self.node_id = NodeID.fromhex(node_id)

        if ipv6 is not None:
            assert NodeIP(ipv6) == self.ipv6, "ipv6 doesn't match provided node_id"

        if name is None:
            self.name = f"d{type(self)._name_counter}"
            type(self)._name_counter += 1
        else:
            self.name = name

        self.image = image
        self.otel = otel

    @property
    def ipv6(self) -> NodeIP:
        return self.node_id.to_node_ip()

    def asdict(self) -> dict[str, str | int]:
        return {
            "node_id": self.node_id.hex(),  # don't store derivable IPv6
            "name": self.name,
            "image": self.image,
            "otel": self.otel,
        }
