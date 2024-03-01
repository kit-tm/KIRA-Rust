# Demo Topology

In this folder are tools to automatically construct and manage topologies.

The tool uses docker images which can be found in [`../docker`](../docker).

To get to know the tool and options just run `topo.py --help`

## Dependencies

- pip modules: `pip install docker networkx`
- [docker](https://www.docker.com/)
  and your build docker image you want to use to setup the network
  (can be achieved by running `make build-images`)
