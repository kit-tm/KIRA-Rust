# Demo Topology

In this folder are tools to automatically construct and manage topologies.

The tool uses docker images which can be found in [`../docker`](../docker).

To get to know the tool and options just run `topo.py --help`

## Dependencies

- pip modules: `pip install docker networkx`
- [docker](https://www.docker.com/)
  and your build docker image you want to use to setup the network
  (can be achieved by running `make build-images`)

## Docker

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

Afterwards restart docker `systemctl restart docker.service`.
