import logging
import os
import re
import docker
from docker.models.networks import Network

from typing import Optional, TypeVar

Self = TypeVar("Self", bound="KIRANode")


logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)


class KIRANode(object):
    def __init__(
        self, client=docker.from_env(), name: str = None, api_port: int = 8080
    ):
        self._client = client

        self._name = name
        self._api_port = api_port
        self._container = None
        self._nid = None

        if name is not None:
            self._adopt_container(name)

    def _adopt_container(self, name: str):
        try:
            self._container = self._client.containers.get(name)
        except docker.errors.NotFound:
            pass

    def api_call(self, path: str, payload: str = None) -> Optional[str]:
        if self._container is None:
            return None

        cmd = f"curl localhost:{self._api_port}/{path}"
        if payload:
            cmd += f" -d {payload}"

        try:
            (_, raw_out) = self._container.exec_run(cmd)
        except docker.errors.APIError:
            return None

        return raw_out.decode("utf-8")

    @property
    def nid(self) -> Optional[bytes]:
        if self._nid is not None:
            return self._nid

        out = self.api_call("node-id")
        if out is None:
            # somehow the API call didn't work
            logger.warn(f"Unable to call node-id from {self._name}")

        match = re.search(r'\{"node-id"*:*"(.+)"\}', out)
        if match is None:
            logger.warn(f"Unable to retrieve node-id from {self._name}")

        # try to retrieve nid from logs
        if match is None:
            logs = self.logs()
            match = re.search(r"Using id (.+)", logs)
            if match is None:
                logger.error(
                    f"Unable to retrieve node-id from {self._name} using the existing logs"
                )
                return None

        nid = match.group(1)
        return bytes.fromhex(nid)

    def create(
        self, img: str, nid: bytes = None, force: bool = False, privileged=False
    ):
        if self._container is not None:
            if not force:
                logger.warn("Node already has a container connected. Not recreating.")
                return
            self.prune()

        environment = []
        # rust log from system environment
        log_level = os.environ.get("RUST_LOG", "debug")
        environment.append(f"RUST_LOG={log_level}")
        if nid is not None:
            nid = nid[:14]
            environment.append(f"NODE_ID={nid.hex()}")
        # TODO: set API port

        self._container = self._client.containers.create(
            image=img,
            name=self._name,
            detach=True,
            privileged=privileged,
            cap_add=["NET_ADMIN"],
            sysctls={
                "net.ipv6.conf.default.disable_ipv6": 0,
                "net.ipv6.conf.eth0.disable_ipv6": 1,  # disable host-interface
                "net.ipv6.conf.all.forwarding": 1,
            },
            environment=environment,
        )
        self._nid = nid
        self._name = self._container.name

    def start(self):
        self._container.start()

    def connect_with(
        self, other: Self, id: str, nw: Optional[Network] = None
    ) -> Network:
        if self._container is None or other._container is None:
            logger.error("Unable to create network between nodes without containers!")
            return None

        fresh = False
        if nw is None:
            fresh = True
            nw = self._client.networks.create(
                name=id,
                driver="bridge",
                options={
                    "com.docker.network.bridge.name": id,
                    "com.docker.network.container_iface_prefix": "r2edge",
                },
                enable_ipv6=True,
            )

        try:
            nw.connect(self._container)
            nw.connect(other._container)
        except docker.errors.APIError as e:
            if fresh:  # only remove nw we created
                nw.remove()
            raise e

        return nw

    def store(self, key: str, data: str) -> str:
        path = f"dht/store?key={key}"
        return self.api_call(path, data)

    def fetch(self, key: str) -> str:
        path = f"dht/fetch?key={key}"
        return self.api_call(path)

    def routing_table(self) -> str:
        path = "_dev/routing-table"
        return self.api_call(path)

    def uln_table(self) -> str:
        path = "_dev/uln-table"
        return self.api_call(path)

    def vicinity_graph(self) -> str:
        path = "_dev/vicinity-graph"
        return self.api_call(path)

    def local_hashtable(self) -> str:
        path = "dht/_dev/local-hashtable"
        return self.api_call(path)

    def ping(self, other: Self, args: str = None) -> bool:
        if args is None:
            args = "-c 3 -i 0.25 -W 1 -q"

        other_nid = other.nid.hex(":", 2)
        cmd = f"ping {args} fc00:{other_nid}"
        # print(cmd)
        (res, _) = self._container.exec_run(cmd)
        return res == 0

    def stop(self):
        if self._container is None:
            return
        self._container.stop()
        self._nid = None

    def prune(self, graceful: bool = False):
        if self._container is None:
            return

        if graceful:
            self.stop()
        self._container.remove(v=True, force=True)

    def logs(self):
        return self._container.logs().decode("utf-8")
