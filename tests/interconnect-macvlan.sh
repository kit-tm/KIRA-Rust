#!/usr/bin/env bash
# script to interconnect a kirad in a namespace to the physical eth interface of a node
namespace=`ip netns list | head -1 | awk '{ print $1 }'`
sudo ip link add kiraout0 link eth0 type macvlan mode private
sudo ip link set kiraout0 netns ${namespace}
sudo ip netns exec ${namespace} ip link set up kiraout0
