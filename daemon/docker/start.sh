#!/bin/sh

# Setup ip6gre interface for encapsulation/decapsulation
ip link add name kira type ip6gre external
ip link set kira up
ip link set eth0 down

sleep 5

./r2kad-daemon