#!/bin/sh

# Usage start.sh <nodeID> <command>
# where command is an arbitrary shell script
nodeID=$1

echo "NodeID is $nodeID."
echo "Setting up forwarding..."

# Local PathIP
localIP=$(/sbin/ip -o -6 a list eth0 | awk '{print $4}' | cut -d/ -f1 | grep fcaa::)

# TODO replace with local address with address not in PathID range

# Setup ip6gre interface for encapsulation/decapsulation
ip link add name kira type ip6gre local $localIP remote beef::1
ip addr add $nodeID/16 dev kira
ip link set kira up

# load nftables rules
echo "define local = $localIP\n" | cat - nftables.conf > temp
mv temp nftables.conf
nft -f nftables.conf

echo "$2" > command.sh
chmod +x command.sh
./command.sh

# TODO add path: in-out-interface (out could be "local" => add to localpaths)
# TODO remove path

# TODO find out if additional pathids must be added as addresses to eth0 or not
