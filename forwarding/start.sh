#!/bin/sh

# Usage start.sh <nodeID>

nodeID=$1

echo "NodeID is $nodeID."
echo "Setting up forwarding..."

# Setup ip6gre interface for encapsulation/decapsulation
ip link add name kira type ip6gre external
# ip addr add $nodeID/16 dev kira
ip link set kira up

# load nftables rules
nft -f nftables.conf