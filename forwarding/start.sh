#!/bin/sh

# Usage start.sh <nodeID> <command>
# where command is an arbitrary shell script
nodeID=$1

echo "NodeID is $nodeID."
echo "Setting up forwarding..."

# Setup ip6gre interface for encapsulation/decapsulation
ip link add name kira type ip6gre local beef::a remote beef::b
# ip addr add $nodeID/16 dev kira
ip link set kira up

# load nftables rules
nft -f nftables.conf

# setup routing policy
ip -6 rule add from all fwmark 0xff00 lookup 65280
ip -6 route add default dev kira table 65280

# setup nodeID
#ip addr add $nodeID/16 dev eth0

# setup GRE interface (why is this necessary?)
#ip addr add beef::a dev eth0
