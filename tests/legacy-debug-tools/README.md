# Demo Topology

In this folder are tools to automatically construct and manage topologies.

All tools can use the provided docker images of the routing daemon
which can be found in [`../docker`](../docker).

To get an overview of the arguments required and optional simply run `demo.py --help` 

The main operations are:

1. `create`: Create docker containers (can be skipped)
2. `connect`: Connect the docker containers
3. `start`: Start all containers
4. `stop`: Stop all containers

## Backends 

There are two backends supported to create the networks.

### Containernet

This backend uses [Containernet](https://containernet.github.io) to build the topology.
To use this backend you need to specify `--containernet` as command-line-argument.

This backend creates, connects and starts the network all in the `start` step.
It is not possible to only "create" the network without connecting it.
Use the docker backend if you want to do so.

#### Dependencies

- installed [Containernet](https://containernet.github.io/#installation)
- pip modules: `pip install docker networkx`
- routing daemon docker image (use `make build-images`)

## Standalone 

This backend does not need any external dependencies apart from docker to run.
In contrast to the Containernet backend, this backend uses vanilla docker
bridge interfaces to bridge the nodes.

#### Dependencies

- [docker](https://www.docker.com/)
- pip modules: `pip install docker networkx`
- routing daemon docker image (use `make build-images`)

#### Docker Setup requirements

> This setup doesn't seem to be required anymore for newer docker versions and can be skipped.

Since the kirad requires IPv6 support you must [enable support for IPv6 in docker](https://docs.docker.com/config/daemon/ipv6/) first:

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
internal examples and documentations by the IANA, which is OK here,
since we don't use them anyway.

Afterwards restart docker with `systemctl restart docker.service`.
