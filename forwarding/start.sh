#!/bin/sh

# Setup ip6gre interface for encapsulation/decapsulation
ip link add name kira type ip6gre external
ip link set kira up

# load nftables rules
nft -f nftables.conf