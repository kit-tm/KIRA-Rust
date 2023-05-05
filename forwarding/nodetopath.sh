#!/bin/sh

ip -6 route add $1/128 encap ip6 dst $2 src fc00::1 dev kira