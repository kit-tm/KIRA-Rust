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
            nid = NodeID.random()
            self.node_id = nid.hex()
            self.ipv6 = nid.to_node_ip().compressed
        else:
            # Ensure validity of provided node_id
            nid = NodeID.fromhex(node_id)
            self.node_id = node_id

        if ipv6 is None:
            self.ipv6 = nid.to_node_ip().compressed
        else:
            assert NodeIP(ipv6) == nid.to_node_ip(), (
                "ipv6 doesn't match provided node_id"
            )
            self.ipv6 = ipv6

        if name is None:
            self.name = f"d{type(self)._name_counter}"
            type(self)._name_counter += 1
        else:
            self.name = name

        self.image = image
        self.otel = otel
