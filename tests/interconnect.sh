#!/usr/bin/env bash
# scrip to interconnect a local kirad into the NeST namespace topology
namespace=`ip netns list | head -1 | awk '{ print $1 }'`
sudo ip link add veth-host type veth peer name veth-nest
sudo ip link set veth-host up
sudo ip link set veth-nest netns ${namespace}
sudo ip netns exec ${namespace} ip link set veth-nest up
