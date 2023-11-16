#!/bin/sh

# Setup ip6gre interface for encapsulation/decapsulation
ip link add name kira type ip6gre external
ip link set kira up
ip link set eth0 down

until [ -f /r2kad-daemon/start ]
do
     sleep 5
done

./r2kad-daemon