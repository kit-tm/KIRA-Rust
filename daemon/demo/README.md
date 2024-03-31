# Demo Topology

In this folder are tools to automatically construct and manage topologies.

All tools can use the provided docker images of the routing daemon
which can be found in [`../docker`](../docker).

## Containernet

This tool uses [Containernet](https://containernet.github.io) to build the topology.

### Dependencies

- installed [Containernet](https://containernet.github.io/#installation)
- pip module networkx for reading the topology file

## Standalone 

This tool does not need any dependencies apart from docker to run.
In contrast to the Containernet based demo this tool uses vanilla docker
bridge interfaces to bridge the nodes.

To get to know the tool and options just run `demo-standalone.py --help`

The main operations are:

1. `create`: Create docker containers (can be skipped)
2. `connect`: Connect the docker containers
3. `start`: Start all containers
4. `stop`: Stop all containers

### Dependencies

- Python >= 3.10 (if running as cli)
- pip modules: `pip install docker networkx`
- [docker](https://www.docker.com/)
- routing daemon docker image (use `make build-images`)

### Docker Setup requirements

Since the r2kad-daemon requires IPv6 support you must [enable support for IPv6 in docker](https://docs.docker.com/config/daemon/ipv6/) first:

Edit `/etc/docker/daemon.json` to at least include `default-address-pools` for IPv6 and `ipv6tables`:

```json
{
    "experimental": true,
    "ip6tables": true,
    "default-address-pools": [
        { "base": "172.17.0.0/12", "size": 20 },
        { "base": "192.168.0.0/16", "size": 24 },
        { "base": "2001:db8::/104", "size": 112 }
    ]
}
```

This is sadly required since it wasn't possible to automatically obtain
link-local IPv6-addresses without also obtaining a global unicast address.

Keep in mind that `2001:db8/32` addresses are only intended to be used in 
internal examples and documentations by the IANA.

Afterwards restart docker with `systemctl restart docker.service`.
