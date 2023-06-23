#!/bin/sh

# Usage start.sh <nodeID>

nodeID=$1

echo "NodeID is $nodeID."
echo "Setting up forwarding..."

# Setup ip6gre interface for encapsulation/decapsulation
ip link add name kira type ip6gre external
ip link set kira up

# load nftables rules
echo "define localNodeIP = $nodeID\n" | cat - nftables.conf > temp
mv temp nftables.conf
nft -f nftables.conf